use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::Result;
use crate::pty::{Pty, PtySpec};
use portable_pty::PtySize;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier};

use crate::vt::{self, ColorRgb};
use crate::xtgettcap::XtgettcapTracker;

/// Catppuccin mocha ANSI 16-color palette.
const PALETTE: [ColorRgb; 16] = [
    ColorRgb::new(0x45, 0x47, 0x5a), // Black
    ColorRgb::new(0xf3, 0x8b, 0xa8), // Red
    ColorRgb::new(0xa6, 0xe3, 0xa1), // Green
    ColorRgb::new(0xf9, 0xe2, 0xaf), // Yellow
    ColorRgb::new(0x89, 0xb4, 0xfa), // Blue
    ColorRgb::new(0xf5, 0xc2, 0xe7), // Magenta
    ColorRgb::new(0x94, 0xe2, 0xd5), // Cyan
    ColorRgb::new(0xba, 0xc2, 0xde), // White
    ColorRgb::new(0x58, 0x5b, 0x70), // BrightBlack
    ColorRgb::new(0xf3, 0x8b, 0xa8), // BrightRed
    ColorRgb::new(0xa6, 0xe3, 0xa1), // BrightGreen
    ColorRgb::new(0xf9, 0xe2, 0xaf), // BrightYellow
    ColorRgb::new(0x89, 0xb4, 0xfa), // BrightBlue
    ColorRgb::new(0xf5, 0xc2, 0xe7), // BrightMagenta
    ColorRgb::new(0x94, 0xe2, 0xd5), // BrightCyan
    ColorRgb::new(0xcd, 0xd6, 0xf4), // BrightWhite
];

pub const FG: RColor = RColor::Rgb(0xcd, 0xd6, 0xf4);
pub const ACCENT: RColor = RColor::Rgb(0xb4, 0xbe, 0xfe);

/// A live pane: PTY + a real terminal emulator (libghostty-vt) fed from it.
#[allow(dead_code)]
pub struct Pane {
    pub id: u64,
    pub session_id: u64,
    pub is_ai: bool,
    /// Whether an AI CLI (opencode/claude) was detected running inside the
    /// pane, even though it was spawned as a plain shell.
    pub detected_ai: bool,
    /// Cached name of the detected AI CLI process (refreshed by the periodic
    /// process scan, so the sidebar never spawns `ps` per frame).
    pub detected_ai_name: Option<String>,
    pub pty: Pty,
    pub vt: vt::Terminal,
    pub dead: bool,
    /// When the pane last produced output. Drives the agent status heuristic.
    last_output: Instant,
    /// Rolling tail of recent output with ANSI escapes stripped, scanned for
    /// blocked (approval) markers.
    recent_text: Vec<u8>,
    /// Stripper state, kept across chunks so escapes split at chunk edges are
    /// still removed.
    stripper: TextStripper,
    /// Whether the viewport changed since the last render (output, selection,
    /// scroll, resize). Lets the frame loop skip re-rendering unchanged panes.
    pub dirty: bool,
    /// Force a full redraw next render. Viewport scroll, resize, and selection
    /// changes do not mark rows dirty in the render state, so they must ignore
    /// the per-row dirty patch.
    pub full_redraw: bool,
    /// Current text of each viewport row, maintained during dirty renders and
    /// used for agent-state detection (no extra render-state scan).
    screen_rows: Vec<String>,
    /// Viewport position where the terminal cursor was last drawn, so the
    /// previous cursor cell can be cleared on a row-level render.
    last_cursor: Option<(u16, u16)>,
    /// Answers terminal capability probes as a plain xterm (no mouse caps).
    xtgettcap: XtgettcapTracker,
}

/// Lifecycle state of an AI (opencode) agent, inferred from its output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentStatus {
    /// Actively producing output (working on a task).
    Working,
    /// Quiet but waiting for a command approval.
    Blocked,
    /// Quiet and idle.
    Idle,
}

/// How much stripped text to keep for blocked-marker scanning. Sized so the
/// opencode permission dialog (a multi-row footer block) survives the quiet
/// period while the agent is paused waiting for approval.
const RECENT_TEXT_BYTES: usize = 16 * 1024;
/// Output markers that indicate the agent is waiting on a command approval.
const BLOCKED_MARKERS: &[&str] = &[
    // opencode permission dialog ("△ Permission required" header + buttons).
    "permission required",
    "allow once",
    "allow always",
    "always allow",
    "reject permission",
    "waiting for permission",
    // Generic approval prompts.
    "do you want to proceed",
    "do you want to run",
    "proceed?",
    "(y/n)",
    "would you like to",
];
/// Markers, scanned against the current screen text, that indicate the agent
/// is actively working. Idle is the fallback when none match (manifest-based
/// detection instead of an output-recently window).
const WORKING_MARKERS: &[&str] = &[
    "esc to interrupt",
    "esc again to interrupt",
    "ctrl+c to interrupt",
    "press esc to interrupt",
    "waiting for assistant",
    "sending prompt",
    "retrying in",
];

#[derive(Clone)]
pub enum PtyEvent {
    Output { pane_id: u64, data: Vec<u8> },
}

/// State of the ANSI escape stripper that builds the marker-scan text buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum AnsiState {
    #[default]
    Normal,
    Esc,
    Csi,
    Osc,
    OscEsc,
    Str,
    StrEsc,
}

/// Strips ANSI escapes (and control bytes) from a byte stream while keeping
/// printable text — including multibyte UTF-8 — intact. Stateful so sequences
/// split across feed chunks are handled correctly.
struct TextStripper {
    state: AnsiState,
    /// Remaining UTF-8 continuation bytes expected after a lead byte.
    cont: u8,
}

impl TextStripper {
    fn new() -> Self {
        Self { state: AnsiState::Normal, cont: 0 }
    }

    /// Append the printable portion of `data` to `out`.
    fn feed(&mut self, data: &[u8], out: &mut Vec<u8>) {
        for &b in data {
            match self.state {
                AnsiState::Normal => {
                    if b == 0x1b {
                        self.state = AnsiState::Esc;
                    } else if b == 0x7f || (b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t') {
                        // Skip control bytes that don't separate words.
                    } else if b >= 0x80 {
                        self.push_utf8(b, out);
                    } else {
                        out.push(b);
                    }
                }
                AnsiState::Esc => match b {
                    b'[' => self.state = AnsiState::Csi,
                    b']' => self.state = AnsiState::Osc,
                    b'P' | b'_' | b'^' | b'X' => self.state = AnsiState::Str,
                    0x1b => self.state = AnsiState::Esc,
                    _ => self.state = AnsiState::Normal,
                },
                AnsiState::Csi => {
                    if b == 0x1b {
                        self.state = AnsiState::Esc;
                    } else if (0x40..=0x7e).contains(&b) {
                        self.state = AnsiState::Normal;
                    }
                }
                AnsiState::Osc => match b {
                    0x1b => self.state = AnsiState::OscEsc,
                    0x07 => self.state = AnsiState::Normal,
                    _ => {}
                },
                AnsiState::OscEsc => {
                    if b == b'\\' {
                        self.state = AnsiState::Normal;
                    } else if b != 0x1b {
                        self.state = AnsiState::Osc;
                    }
                }
                AnsiState::Str => match b {
                    0x1b => self.state = AnsiState::StrEsc,
                    0x9c => self.state = AnsiState::Normal,
                    _ => {}
                },
                AnsiState::StrEsc => {
                    if b == b'\\' {
                        self.state = AnsiState::Normal;
                    } else if b != 0x1b {
                        self.state = AnsiState::Str;
                    }
                }
            }
        }
    }

    /// Accept a byte >= 0x80 as UTF-8 text or skip it as a C1 control.
    fn push_utf8(&mut self, b: u8, out: &mut Vec<u8>) {
        if self.cont > 0 && (0x80..=0xbf).contains(&b) {
            out.push(b);
            self.cont -= 1;
        } else if (0xc2..=0xf4).contains(&b) {
            out.push(b);
            self.cont = match b {
                0xc2..=0xdf => 1,
                0xe0..=0xef => 2,
                _ => 3,
            };
        } else {
            // Standalone C1 control or a stray continuation byte: drop it.
            self.cont = 0;
        }
    }
}

impl Pane {
    pub fn spawn(
        session_id: u64,
        id: u64,
        shell: String,
        program: Option<(String, Vec<String>)>,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        is_ai: bool,
        events_tx: Sender<PtyEvent>,
    ) -> Result<Pane> {
        let pty = Pty::spawn(&PtySpec {
            shell,
            program,
            cwd,
            cols,
            rows,
        })?;

        let vt = vt::Terminal::new(cols.max(1), rows.max(1), 10_000, &PALETTE)?;

        let mut pane = Pane {
            id,
            session_id,
            is_ai,
            detected_ai: false,
            detected_ai_name: None,
            pty,
            vt,
            dead: false,
            last_output: Instant::now(),
            recent_text: Vec::with_capacity(256),
            stripper: TextStripper::new(),
            dirty: true,
            full_redraw: true,
            screen_rows: Vec::new(),
            last_cursor: None,
            xtgettcap: XtgettcapTracker::new(),
        };

        // Route query responses (DA, size reports, etc.) back to the PTY.
        pane.vt.set_write_sink(&mut *pane.pty.writer as *mut (dyn std::io::Write + Send));

        let reader = pane.pty.master.try_clone_reader()?;
        let tx = events_tx.clone();
        let pid = id;
        Pty::read_loop(reader, move |data| {
            let _ = tx.send(PtyEvent::Output { pane_id: pid, data });
        });

        Ok(pane)
    }

    pub fn feed(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.last_output = Instant::now();
            self.dirty = true;
            self.stripper.feed(data, &mut self.recent_text);
            if self.recent_text.len() > RECENT_TEXT_BYTES {
                let drop = self.recent_text.len() - RECENT_TEXT_BYTES;
                self.recent_text.drain(..drop);
            }
            // Answer XTGETTCAP capability probes as a plain xterm so apps
            // don't enable mouse reporting and steal text selection.
            self.xtgettcap.observe(data);
            for response in self.xtgettcap.drain_pending() {
                let _ = self.pty.write(&response);
            }
        }
        self.vt.write(data);
    }

    /// Whether this pane counts as an AI CLI pane: either spawned as one, or
    /// an AI CLI (opencode/claude) was detected running inside a plain shell.
    pub fn is_ai_cli(&self) -> bool {
        self.is_ai || self.detected_ai
    }

    /// Name of the AI CLI currently running in this pane's process tree.
    pub fn ai_cli_name(&self) -> Option<String> {
        let root = self.pty.child.process_id()?;
        ProcessSnapshot::capture()?.ai_cli_in_tree(root)
    }

    /// Milliseconds since the pane last produced output. Debug/diagnostics.
    pub fn last_output_age(&self) -> Duration {
        self.last_output.elapsed()
    }

    /// Last `max_chars` of stripped output text. Debug/diagnostics.
    pub fn recent_text_tail(&self, max_chars: usize) -> String {
        let start = self.recent_text.len().saturating_sub(max_chars);
        String::from_utf8_lossy(&self.recent_text[start..]).into_owned()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        self.vt.resize(cols, rows);
        self.dirty = true;
        self.full_redraw = true;
        let _ = self.pty.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    pub fn write(&mut self, data: &[u8]) {
        let _ = self.pty.write(data);
    }

    pub fn scroll(&mut self, delta: i32) {
        self.vt.scroll(delta);
        self.dirty = true;
        self.full_redraw = true;
    }

    pub fn has_mouse_reporting(&self) -> bool {
        self.vt.mouse_tracking()
    }

    /// Whether the alternate screen is active.
    pub fn in_alt_screen(&self) -> bool {
        self.vt.mode_get(vt::MODE_ALT_SCREEN)
    }

    /// Scrollbar state (total/offset/len rows), refreshed during `render`.
    pub fn scrollbar_data(&self) -> vt::TerminalScrollbar {
        self.vt.scrollbar()
    }

    /// Re-render only the rows that changed since the last frame into `cache`
    /// (the pane's retained viewport buffer), skipping clean rows via the
    /// render-state dirty patch. Maintains `screen_rows` for agent-state
    /// detection. Returns the agent status when this is an AI CLI pane.
    pub fn render_dirty(
        &mut self,
        area: Rect,
        focused: bool,
        cache: &mut Buffer,
    ) -> Option<AgentStatus> {
        let area_x = area.x;
        let area_y = area.y;
        let aw = area.width as i32;
        let ah = area.height as i32;

        self.vt.refresh();
        let level = self.vt.render_dirty_level();
        // Blank cells use the host terminal's default background (Reset), so
        // panes look native; apps that paint their own background (e.g. a
        // black opencode) keep theirs via `has_bg`.
        let fg = RColor::Reset;
        let bg = RColor::Reset;
        let full = level == vt::DIRTY_FULL || self.full_redraw;
        self.full_redraw = false;

        let mut dirty: HashSet<usize> = if full {
            (0..ah.max(0) as usize).collect()
        } else {
            self.vt.dirty_rows().into_iter().collect()
        };

        // Always re-render the cursor's row and the previous cursor's row so
        // the inverted cursor is drawn/cleared even on an otherwise clean row.
        if focused && self.vt.cursor_visible() {
            if let Some((_, cy)) = self.vt.cursor_pos() {
                dirty.insert(cy as usize);
            }
        }
        if let Some((_, oy)) = self.last_cursor {
            dirty.insert(oy as usize);
        }
        self.last_cursor = if focused && self.vt.cursor_visible() {
            self.vt.cursor_pos()
        } else {
            None
        };

        // Reset dirty rows to the default background, then apply populated
        // cells so content that disappeared this frame is cleared.
        for y in dirty.iter().copied() {
            let yy = area_y + y as u16;
            if yy >= area_y + area.height {
                continue;
            }
            for x in area_x..(area_x + area.width) {
                if let Some(c) = cache.cell_mut((x, yy)) {
                    c.set_char(' ').set_fg(fg).set_bg(bg);
                    c.modifier = Modifier::empty();
                }
            }
        }

        self.vt.for_each_cell(|row, col, rc, selected, row_dirty| {
            if !full && !row_dirty && !dirty.contains(&row) {
                return;
            }
            if row >= ah as usize || col >= aw as usize {
                return;
            }
            let bcell = cache.cell_mut((area_x + col as u16, area_y + row as u16)).unwrap();
            let mut mods = Modifier::empty();
            if rc.bold {
                mods |= Modifier::BOLD;
            }
            if rc.italic {
                mods |= Modifier::ITALIC;
            }
            if rc.underline {
                mods |= Modifier::UNDERLINED;
            }
            if rc.inverse {
                mods |= Modifier::REVERSED;
            }
            if rc.faint {
                mods |= Modifier::DIM;
            }
            if selected {
                mods |= Modifier::REVERSED;
            }
            // Respect explicit app colors; otherwise fall back to the host
            // terminal's defaults (native).
            let cell_fg = if rc.has_fg { rgb(rc.fg) } else { RColor::Reset };
            let cell_bg = if rc.has_bg { rgb(rc.bg) } else { RColor::Reset };
            if rc.text.is_empty() {
                bcell.set_char(' ').set_fg(cell_fg).set_bg(cell_bg);
            } else {
                let ch = rc.text.chars().next().unwrap_or(' ');
                bcell.set_char(ch).set_fg(cell_fg).set_bg(cell_bg);
            }
            bcell.modifier = mods;
        });

        if focused && self.vt.cursor_visible() {
            if let Some((cx, cy)) = self.vt.cursor_pos() {
                let x = area_x + cx;
                let y = area_y + cy;
                if x < area_x + area.width && y < area_y + area.height {
                    if let Some(bcell) = cache.cell_mut((x, y)) {
                        let f = bcell.fg;
                        let b = bcell.bg;
                        bcell.set_fg(b).set_bg(f);
                        bcell.modifier = Modifier::REVERSED;
                    }
                }
            }
        }

        self.update_screen_rows(area, cache, &dirty);

        self.vt.clear_dirty();
        self.dirty = false;

        if self.is_ai_cli() {
            Some(self.compute_agent_status())
        } else {
            None
        }
    }

    /// Rebuild the text of the given rows from `cache`, for agent detection.
    fn update_screen_rows(&mut self, area: Rect, cache: &Buffer, dirty: &HashSet<usize>) {
        let rows = area.height as usize;
        if self.screen_rows.len() < rows {
            self.screen_rows.resize(rows, String::new());
        }
        for &y in dirty {
            if y >= rows {
                continue;
            }
            let mut line = String::new();
            for x in area.x..(area.x + area.width) {
                if let Some(cell) = cache.cell((x, area.y + y as u16)) {
                    line.push_str(cell.symbol());
                }
            }
            self.screen_rows[y] = line.trim_end().to_string();
        }
    }

    /// Agent lifecycle state, derived from the pane's screen rows: Blocked/
    /// Working win via distinctive markers, Idle is the fallback.
    pub fn agent_status(&self) -> AgentStatus {
        self.compute_agent_status()
    }

    fn compute_agent_status(&self) -> AgentStatus {
        if self.dead {
            return AgentStatus::Idle;
        }
        let mut screen = String::new();
        for line in &self.screen_rows {
            screen.push_str(line);
            screen.push('\n');
        }
        let lower = screen.to_lowercase();
        if BLOCKED_MARKERS.iter().any(|m| lower.contains(m)) {
            return AgentStatus::Blocked;
        }
        if WORKING_MARKERS.iter().any(|m| lower.contains(m)) {
            return AgentStatus::Working;
        }
        // opencode's knight-rider status bar: 4+ block cells in a row.
        if ["■■■■", "⬝⬝⬝⬝"].iter().any(|p| screen.contains(p)) {
            return AgentStatus::Working;
        }
        // Braille spinner (tool call / thinking) visible on screen.
        if screen.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)) {
            return AgentStatus::Working;
        }
        AgentStatus::Idle
    }

    /// Install the terminal's active selection from two viewport coordinates.
    pub fn set_selection(&mut self, start: (u16, u16), end: (u16, u16)) -> bool {
        let ok = self.vt.set_selection(start, end);
        if ok {
            self.dirty = true;
            self.full_redraw = true;
        }
        ok
    }

    /// Clear the terminal's active selection.
    pub fn clear_selection(&mut self) {
        self.vt.clear_selection();
        self.dirty = true;
        self.full_redraw = true;
    }

    /// Extract the text between two viewport coordinates as plain text,
    /// building a fresh selection at extraction time (unwrap + trim).
    pub fn selection_text(&mut self, start: (u16, u16), end: (u16, u16)) -> Option<String> {
        self.vt.selection_text(start, end)
    }

    /// Viewport position of the terminal cursor, relative to the pane origin.
    /// Requires a prior `render` (which refreshes the render state).
    pub fn cursor_pos(&self) -> Option<(u16, u16)> {
        if self.vt.mode_get(vt::MODE_CURSOR_VISIBLE) {
            self.vt.cursor_pos()
        } else {
            None
        }
    }
}

fn rgb(c: ColorRgb) -> RColor {
    RColor::Rgb(c.r, c.g, c.b)
}

pub fn sgr_mouse(button: u8, col: u16, row: u16, release: bool) -> Vec<u8> {
    let b = if release { button | 3 } else { button };
    format!("\x1b[<{b};{col};{row}{}\x1b[0m", if release { "m" } else { "M" }).into_bytes()
}

/// Executable names treated as AI CLI panes.
const AI_CLI_NAMES: &[&str] = &[
    "opencode", "claude", "codex", "gemini", "qwen", "aider", "cody", "swe", "coco",
];

/// Snapshot of the process table (parent/child map + executable names), used
/// to detect whether an AI CLI process runs inside a pane's process tree.
pub struct ProcessSnapshot {
    children: HashMap<u32, Vec<u32>>,
    names: HashMap<u32, String>,
}

impl ProcessSnapshot {
    /// Capture the current process table, or `None` when unavailable.
    pub fn capture() -> Option<ProcessSnapshot> {
        #[cfg(target_os = "windows")]
        {
            None
        }
        #[cfg(not(target_os = "windows"))]
        {
            let out = std::process::Command::new("ps")
                .args(["-axo", "pid=,ppid=,comm="])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
            let mut names: HashMap<u32, String> = HashMap::new();
            for line in out.stdout.split(|&b| b == b'\n') {
                let mut fields = line.split(|&b| b == b' ').filter(|f| !f.is_empty());
                let (Some(pid), Some(ppid)) = (fields.next(), fields.next()) else {
                    continue;
                };
                let (Some(pid), Some(ppid)) = (
                    std::str::from_utf8(pid).ok().and_then(|s| s.parse().ok()),
                    std::str::from_utf8(ppid).ok().and_then(|s| s.parse().ok()),
                ) else {
                    continue;
                };
                let name = fields
                    .map(|f| String::from_utf8_lossy(f).into_owned())
                    .collect::<Vec<_>>()
                    .join(" ");
                children.entry(ppid).or_default().push(pid);
                names.insert(pid, name);
            }
            Some(ProcessSnapshot { children, names })
        }
    }

    /// The AI CLI process name reachable from `root`, if any.
    pub fn ai_cli_in_tree(&self, root: u32) -> Option<String> {
        let mut stack = vec![root];
        let mut seen = HashSet::new();
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(name) = self.names.get(&pid) {
                let base = name.rsplit('/').next().unwrap_or(name);
                if AI_CLI_NAMES.contains(&base) {
                    return Some(name.clone());
                }
            }
            if let Some(kids) = self.children.get(&pid) {
                stack.extend(kids);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::sync::mpsc;

    fn test_pane(is_ai: bool) -> Pane {
        let (tx, _rx) = mpsc::channel();
        Pane::spawn(
            1,
            1,
            "/bin/sh".into(),
            Some(("/usr/bin/true".into(), Vec::new())),
            None,
            120,
            40,
            is_ai,
            tx,
        )
        .unwrap()
    }

    /// Feed already happened; render the pane once so `screen_rows` (the
    /// agent-status source) reflects the fed output.
    fn render_pane(p: &mut Pane) {
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        p.render_dirty(area, true, &mut buf);
    }

    /// Text of each row in a rendered pane buffer.
    fn pane_text(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    line.push_str(cell.symbol());
                }
            }
            let line = line.trim_end();
            if !line.is_empty() {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn working_after_recent_output() {
        let mut p = test_pane(true);
        p.feed(b"working on it - esc to interrupt");
        render_pane(&mut p);
assert_eq!(p.agent_status(), AgentStatus::Working);
    }

    #[test]
    fn idle_after_quiet_period() {
        let mut p = test_pane(true);
        p.feed(b"finished the task");
        p.last_output = Instant::now() - Duration::from_secs(10);
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Idle);
    }

    #[test]
    fn blocked_when_quiet_and_waiting_approval() {
        let mut p = test_pane(true);
        p.feed(b"Do you want to proceed with this command? (y/n)");
        p.last_output = Instant::now() - Duration::from_secs(10);
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Blocked);
    }

    #[test]
    fn blocked_while_spinner_keeps_repainting() {
        let mut p = test_pane(true);
        p.feed(b"\x1b[0m\xe2\x96\xb3 Permission required\nAllow once\nAllow always\nReject");
        render_pane(&mut p);
assert_eq!(p.agent_status(), AgentStatus::Blocked);
    }

    #[test]
    fn blocked_detected_through_ansi_fragmentation() {
        let mut p = test_pane(true);
        // opencode wraps words in SGR sequences, so the raw buffer would never
        // contain the contiguous marker text.
        p.feed(b"\x1b[38;2;255;100;100mPermission\x1b[0m required\n");
        p.feed(b"\x1b[1mAllow\x1b[22m once\n");
        render_pane(&mut p);
assert_eq!(p.agent_status(), AgentStatus::Blocked);
    }

    #[test]
    fn stripper_handles_split_escapes() {
        let mut s = TextStripper::new();
        let mut out = Vec::new();
        s.feed(b"hello \x1b[3", &mut out);
        s.feed(b"1mworld\x1b[0m", &mut out);
        assert_eq!(String::from_utf8_lossy(&out), "hello world");
    }

    #[test]
    fn stripper_keeps_multibyte_utf8() {
        let mut s = TextStripper::new();
        let mut out = Vec::new();
        s.feed("△ Permission".as_bytes(), &mut out);
        assert_eq!(String::from_utf8_lossy(&out), "△ Permission");
    }

    #[test]
    fn stripper_skips_osc_strings() {
        let mut s = TextStripper::new();
        let mut out = Vec::new();
        s.feed(b"a\x1b]52;c;dGVzdA==\x07b", &mut out);
        assert_eq!(String::from_utf8_lossy(&out), "ab");
    }

    #[test]
    fn working_when_agent_streams_recent_output() {
        let mut p = test_pane(true);
        p.feed(b"\xe2\x96\xa0\xe2\x96\xa0\xe2\x96\xa0\xe2\x96\xa0running...");
        render_pane(&mut p);
assert_eq!(p.agent_status(), AgentStatus::Working);
    }

    #[test]
    fn idle_when_screen_has_no_working_marker() {
        let mut p = test_pane(true);
        p.feed(b"opencode 1.18.15\n~/.opencode\n");
        render_pane(&mut p);
assert_eq!(p.agent_status(), AgentStatus::Idle);
    }

    #[test]
    fn first_render_after_initial_output_shows_all_lines() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        // Fish prints its banner immediately; the FIRST render happens after.
        p.feed(b"Welcome to fish, the friendly interactive shell.\r\nfish, version 4.6.0\r\n> ");
        p.render_dirty(area, true, &mut buf);
        let t = pane_text(&buf);
        assert!(t.contains("Welcome to fish"), "banner lost on first render: {t:?}");
        assert!(t.contains("fish, version 4.6.0"), "banner line lost: {t:?}");
    }

    #[test]
    fn default_cells_use_native_terminal_background() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        p.feed(b"hi");
        p.render_dirty(area, true, &mut buf);
        // Text with no explicit color and blank areas fall back to the host
        // terminal's default (Reset), not a fixed kumo background.
        assert_eq!(buf.cell((0, 0)).unwrap().bg, RColor::Reset, "text cell bg");
        assert_eq!(buf.cell((60, 30)).unwrap().bg, RColor::Reset, "blank cell bg");
    }

    #[test]
    fn opencode_raw_output_renders_without_losing_rows() {        let raw = match std::fs::read("/tmp/oc_msg.raw") {
            Ok(d) => d,
            Err(_) => return, // captured fixture not present
        };
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        // Feed like a pty: many small chunks, one render after each burst.
        for chunk in raw.chunks(256) {
            p.feed(chunk);
            p.render_dirty(area, true, &mut buf);
        }
        let text = pane_text(&buf);
        // opencode's composer + footer should be visible after a full session.
        assert!(text.contains("say hello"), "opencode message lost: {text:?}");
    }

    #[test]
    fn partial_render_preserves_static_rows() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);

        p.feed(b"line one\nline two\n");
        p.render_dirty(area, true, &mut buf);
        assert!(pane_text(&buf).contains("line one"), "initial render lost content");

        // A later change to a different area must not wipe earlier rows.
        p.feed(b"\x1b[5;1Hline three\n");
        p.render_dirty(area, true, &mut buf);
        let text = pane_text(&buf);
        assert!(text.contains("line one"), "static row lost: {text:?}");
        assert!(text.contains("line two"), "static row lost: {text:?}");
        assert!(text.contains("line three"), "new row missing: {text:?}");
    }

    #[test]
    fn clear_screen_clears_rows() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        p.feed(b"aaa\nbbb\n");
        p.render_dirty(area, true, &mut buf);
        assert!(pane_text(&buf).contains("aaa"));
        p.feed(b"\x1b[2J\x1b[H");
        p.render_dirty(area, true, &mut buf);
        let t = pane_text(&buf);
        assert!(!t.contains("aaa"), "clear screen left content: {t:?}");
    }

    #[test]
    fn cursor_toggle_keeps_content() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        p.feed(b"hello world");
        p.render_dirty(area, true, &mut buf);
        assert!(pane_text(&buf).contains("hello world"));
        // Show cursor, hide cursor, more output elsewhere.
        p.feed(b"\x1b[?25h\x1b[?25l\x1b[3;1Hnew line");
        p.render_dirty(area, true, &mut buf);
        let t = pane_text(&buf);
        assert!(t.contains("hello world"), "content lost after cursor toggles: {t:?}");
        assert!(t.contains("new line"));
    }

    #[test]
    fn render_before_output_then_feed_shows_all_rows() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        // Empty render first (like a freshly spawned pane on frame 1).
        p.render_dirty(area, true, &mut buf);
        p.feed(b"hello\nworld\nthird");
        p.render_dirty(area, true, &mut buf);
        let t = pane_text(&buf);
        assert!(t.contains("hello"), "content lost after empty first render: {t:?}");
        assert!(t.contains("world"), "content lost after empty first render: {t:?}");
        assert!(t.contains("third"), "content lost after empty first render: {t:?}");
    }

    #[test]
    fn scroll_up_redraws_visible_rows() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        let mut s = String::new();
        for i in 0..60 {
            s.push_str(&format!("row {i}\r\n"));
        }
        p.feed(s.as_bytes());
        p.render_dirty(area, true, &mut buf);
        assert!(pane_text(&buf).contains("row 59"), "initial scrollback render wrong");

        p.scroll(-10);
        p.render_dirty(area, true, &mut buf);
        let row0: String = (0..16).map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()).unwrap_or_default()).collect();
        assert!(row0.starts_with("row 1"), "top row after scroll wrong: {row0:?}");
        let t = pane_text(&buf);
        assert!(t.contains("row 11"), "scroll did not redraw rows: {t:?}");
        assert!(t.contains("row 50"), "scrolled rows missing: {t:?}");
        assert!(!t.contains("row 59"), "bottom row should be off-screen: {t:?}");
    }

    #[test]
    fn select_text_in_opencode() {
        let raw = std::fs::read("/tmp/oc_msg.raw").unwrap();
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        for chunk in raw.chunks(256) {
            p.feed(chunk);
            p.render_dirty(area, true, &mut buf);
        }
        // Locate "say hello" in the rendered viewport.
        let lines: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()).unwrap_or_default())
                    .collect()
            })
            .collect();
        let mut at = None;
        for (y, line) in lines.iter().enumerate() {
            if let Some(byte_off) = line.find("say hello") {
                let char_off = line[..byte_off].chars().count();
                at = Some((char_off as u16, y as u16));
                break;
            }
        }
        let (sx, sy) = at.expect("say hello on screen");
        let (ex, ey) = (sx + "say hello".len() as u16 - 1, sy);
        assert!(p.set_selection((sx, sy), (ex, ey)), "set_selection failed");
        let text = p.selection_text((sx, sy), (ex, ey)).unwrap_or_default();
        assert!(text.contains("say hello"), "selection wrong: {text:?}");

        // Highlight must cover exactly the selected text cells (plus the
        // terminal cursor, which is also reversed).
        p.render_dirty(area, true, &mut buf);
        let hl = |x: u16, y: u16| buf.cell((x, y)).unwrap().modifier.contains(Modifier::REVERSED);
        for dx in 0.."say hello".len() as u16 {
            assert!(hl(sx + dx, sy), "selected cell not highlighted ({},{})", sx + dx, sy);
        }
        assert!(!hl(0, sy), "cell before selection highlighted");
        assert!(!hl(sx + "say hello".len() as u16, sy), "cell after selection highlighted");

        // Multi-row selection returns both lines joined.
        p.clear_selection();
        let (sx2, sy2) = (0u16, 0u16);
        assert!(p.set_selection((sx2, sy2), (20, 3)), "multi set_selection failed");
        let mtext = p.selection_text((sx2, sy2), (20, 3)).unwrap_or_default();
        assert!(mtext.contains("say hello"), "multi-row selection lost text: {mtext:?}");
    }

    #[test]
    fn process_snapshot_finds_ai_cli_in_subtree() {
        let snap = ProcessSnapshot {
            children: HashMap::from([(1, vec![2, 3]), (2, vec![4])]),
            names: HashMap::from([
                (1, "zsh".to_string()),
                (2, "bash".to_string()),
                (3, "other".to_string()),
                (4, "opencode".to_string()),
            ]),
        };
        assert_eq!(snap.ai_cli_in_tree(1), Some("opencode".to_string()));
        assert_eq!(snap.ai_cli_in_tree(4), Some("opencode".to_string()));
        assert!(snap.ai_cli_in_tree(3).is_none());
    }

    #[test]
    fn process_snapshot_matches_comm_basename() {
        let snap = ProcessSnapshot {
            children: HashMap::from([(1, vec![2])]),
            names: HashMap::from([
                (1, "zsh".to_string()),
                (2, "/Users/x/.opencode/bin/opencode".to_string()),
            ]),
        };
        assert_eq!(snap.ai_cli_in_tree(1), Some("/Users/x/.opencode/bin/opencode".to_string()));
    }

    #[test]
    fn process_snapshot_capture_parses_real_ps() {
        let snap = ProcessSnapshot::capture();
        let snap = snap.expect("ps should run on this platform");
        assert!(!snap.children.is_empty());
        assert!(!snap.names.is_empty());
    }
}
