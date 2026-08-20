use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::Result;
use crate::daemon::agents::AgentStatus;
use crate::daemon::pty::{Pty, PtySpec};
use kumo_core::theme::{OwnedTheme, Theme};
use ratatui::buffer::{Buffer, CellDiffOption, CellWidth};
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier};

use crate::daemon::vt::{self, find_urls};
use kumo_core::color::ColorRgb;
use crate::daemon::xtgettcap::XtgettcapTracker;

/// A live pane: PTY + a real terminal emulator (libghostty-vt) fed from it.
#[allow(dead_code)]
pub struct Pane {
    pub id: u64,
    pub session_id: u64,
    pub is_ai: bool,
    /// Program + args this pane was spawned with (AI panes). None = plain shell.
    pub program: Option<(String, Vec<String>)>,
    /// Working directory the pane was spawned in.
    pub cwd: PathBuf,
    /// Custom pane name set by the user (rename). Overrides the default
    /// `shell N` / `AI CLI` label in the pane title.
    pub custom_name: Option<String>,
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
    recent_text_start: usize,
    recent_text_len: usize,
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
    /// Viewport position where the terminal cursor was last drawn, so the
    /// previous cursor cell can be cleared on a row-level render.
    last_cursor: Option<(u16, u16)>,
    /// Answers terminal capability probes as a plain xterm (no mouse caps).
    xtgettcap: XtgettcapTracker,
    /// Whether a link modifier (Cmd/Ctrl/Option) is currently held. Links are
    /// underlined only while it is set, so holding Cmd shows what's clickable.
    pub link_mods: bool,
}

/// How much stripped text to keep for blocked-marker scanning. Sized so the
/// opencode permission dialog (a multi-row footer block) survives the quiet
/// period while the agent is paused waiting for approval.
const RECENT_TEXT_BYTES: usize = 16 * 1024;

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
    // Spawn/resume/finish take every `Pane` init parameter as a flat argument
    // list (they are all distinct, with no natural subgrouping worth a struct);
    // clippy's threshold doesn't fit them, so the lint is disabled here.
    #[allow(clippy::too_many_arguments)]
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
        theme: &OwnedTheme,
    ) -> Result<Pane> {
        let pty = Pty::spawn(&PtySpec {
            shell: shell.clone(),
            program: program.clone(),
            cwd: cwd.clone(),
            cols,
            rows,
        })?;
        Self::finish(
            id,
            session_id,
            is_ai,
            program,
            cwd.unwrap_or_else(|| PathBuf::from("/")),
            cols,
            rows,
            pty,
            events_tx,
            theme,
        )
    }

    /// Spawn with a built-in `Theme` static (for tests). Delegates to `spawn` via conversion.
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    pub fn spawn_with_theme(
        session_id: u64,
        id: u64,
        shell: String,
        program: Option<(String, Vec<String>)>,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        is_ai: bool,
        events_tx: Sender<PtyEvent>,
        theme: &Theme,
    ) -> Result<Pane> {
        let owned = OwnedTheme::from(*theme);
        Self::spawn(session_id, id, shell, program, cwd, cols, rows, is_ai, events_tx, &owned)
    }

    /// Adopt a PTY master inherited across a daemon restart (`kumo update`):
    /// the child process is still alive inside the PTY, so this pane comes up
    /// with a fresh (blank) terminal emulator connected to the live process.
    /// `mouse_tracking` is the pane's DEC mouse-reporting state as snapshotted
    /// before the restart: the app never saw the emulator reset, so its own
    /// mouse mode is unchanged app-side and the fresh emulator must re-learn
    /// it, or kumo would stop forwarding the mouse to the app.
    #[cfg(unix)]
    #[allow(clippy::too_many_arguments)]
    pub fn resume(
        session_id: u64,
        id: u64,
        shell: String,
        program: Option<(String, Vec<String>)>,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
        is_ai: bool,
        master_fd: i32,
        child_pid: Option<i32>,
        mouse_tracking: bool,
        snapshot: Option<Vec<u8>>,
        events_tx: Sender<PtyEvent>,
        theme: &OwnedTheme,
    ) -> Result<Pane> {
        // Try snapshot first: decode the terminal before creating the PTY so we
        // can fallback to a blank terminal without losing the PTY fd.
        let (mut pane, is_snapshot) = if let Some(bytes) = snapshot {
            match vt::Terminal::from_snapshot(
                &bytes,
                &theme.palette,
                theme.term_fg,
                theme.term_bg,
                theme.term_cursor,
            ) {
                Ok(vt) => {
                    let pty = Pty::resume(id, master_fd, child_pid, cols.max(1), rows.max(1), shell.clone())?;
                    let pane = Self::build_pane(id, session_id, is_ai, program.clone(), cwd.clone(), pty, vt, events_tx.clone())?;
                    (pane, true)
                }
                Err(e) => {
                    log::warn!("kumo: snapshot decode failed for pane {id}: {e:#}, falling back to blank");
                    let pty = Pty::resume(id, master_fd, child_pid, cols.max(1), rows.max(1), shell)?;
                    let pane = Self::finish(id, session_id, is_ai, program, cwd, cols, rows, pty, events_tx, theme)?;
                    (pane, false)
                }
            }
        } else {
            let pty = Pty::resume(id, master_fd, child_pid, cols.max(1), rows.max(1), shell)?;
            let pane = Self::finish(id, session_id, is_ai, program, cwd, cols, rows, pty, events_tx, theme)?;
            (pane, false)
        };
        if mouse_tracking {
            pane.vt.mode_set(vt::MODE_MOUSE_NORMAL, true);
        }
        if !is_snapshot {
            // Blank fallback: nudge to force repaint (see original comment).
            pane.resize(cols.saturating_sub(1).max(1), rows);
            pane.resize(cols.max(1), rows.max(1));
            let _ = pane.pty.resize(cols.saturating_sub(1).max(1), rows);
        } else {
            // Snapshot case: ensure PTY size matches, no nudge needed.
            let _ = pane.pty.resize(cols.max(1), rows.max(1));
            pane.resize(cols.max(1), rows.max(1));
        }
        Ok(pane)
    }

    /// Shared tail of `spawn`/`resume`: create the terminal emulator, wire the
    /// PTY writer as its query-response sink, and start the read loop.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        id: u64,
        session_id: u64,
        is_ai: bool,
        program: Option<(String, Vec<String>)>,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
        pty: Pty,
        events_tx: Sender<PtyEvent>,
        theme: &OwnedTheme,
    ) -> Result<Pane> {
        let vt = vt::Terminal::new(
            cols.max(1),
            rows.max(1),
            10_000,
            &theme.palette,
            theme.term_fg,
            theme.term_bg,
            theme.term_cursor,
        )?;
        Self::build_pane(id, session_id, is_ai, program, cwd, pty, vt, events_tx)
    }

    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    fn finish_from_snapshot(
        id: u64,
        session_id: u64,
        is_ai: bool,
        program: Option<(String, Vec<String>)>,
        cwd: PathBuf,
        pty: Pty,
        snapshot: Vec<u8>,
        events_tx: Sender<PtyEvent>,
        theme: &OwnedTheme,
    ) -> Result<Pane> {
        let vt = vt::Terminal::from_snapshot(
            &snapshot,
            &theme.palette,
            theme.term_fg,
            theme.term_bg,
            theme.term_cursor,
        )
        .map_err(|e| anyhow::anyhow!("snapshot decode failed: {e:#}"))?;
        Self::build_pane(id, session_id, is_ai, program, cwd, pty, vt, events_tx)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_pane(
        id: u64,
        session_id: u64,
        is_ai: bool,
        program: Option<(String, Vec<String>)>,
        cwd: PathBuf,
        pty: Pty,
        vt: vt::Terminal,
        events_tx: Sender<PtyEvent>,
    ) -> Result<Pane> {
        let mut pane = Pane {
            id,
            session_id,
            is_ai,
            program,
            cwd,
            custom_name: None,
            detected_ai: false,
            detected_ai_name: None,
            pty,
            vt,
            dead: false,
            last_output: Instant::now(),
            recent_text: Vec::with_capacity(RECENT_TEXT_BYTES),
            recent_text_start: 0,
            recent_text_len: 0,
            stripper: TextStripper::new(),
            dirty: true,
            full_redraw: true,
            last_cursor: None,
            xtgettcap: XtgettcapTracker::new(),
            link_mods: false,
        };

        // Route query responses (DA, size reports, etc.) back to the PTY.
        pane.vt.set_write_sink(&mut *pane.pty.writer as *mut (dyn std::io::Write + Send));

        let reader = pane.pty.reader()?;
        let tx = events_tx.clone();
        let pid = id;
        Pty::read_loop(reader, move |data| {
            let _ = tx.send(PtyEvent::Output { pane_id: pid, data });
        });

        Ok(pane)
    }

    /// Recolor the terminal emulator for a newly selected theme. Forces a full
    /// redraw so the new background/palette reach the next render.
    #[allow(dead_code)]
    pub fn apply_theme(&mut self, theme: &Theme) {
        self.vt.apply_theme(&theme.palette, theme.term_fg, theme.term_bg, theme.term_cursor);
        self.dirty = true;
        self.full_redraw = true;
    }

    pub fn apply_theme_owned(&mut self, theme: &OwnedTheme) {
        self.vt.apply_theme(&theme.palette, theme.term_fg, theme.term_bg, theme.term_cursor);
        self.dirty = true;
        self.full_redraw = true;
    }

    pub fn feed(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.last_output = Instant::now();
            self.dirty = true;
            self.stripper.feed(data, &mut self.recent_text);
            self.recent_text_len = self.recent_text.len() - self.recent_text_start;
            if self.recent_text_len > RECENT_TEXT_BYTES {
                let drop = self.recent_text_len - RECENT_TEXT_BYTES;
                self.recent_text_start += drop;
                self.recent_text_len = RECENT_TEXT_BYTES;
                if self.recent_text_start > RECENT_TEXT_BYTES {
                    self.recent_text.copy_within(self.recent_text_start.., 0);
                    self.recent_text.truncate(self.recent_text_len);
                    self.recent_text_start = 0;
                }
            }
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

    /// The current working directory this pane is *actually* in, used by
    /// follow-workspace. Priority: the **foreground process group leader's**
    /// cwd (what is running in the terminal right now — the shell at a prompt
    /// or the active command), then the shell-reported pwd (OSC 7 / OSC 9 /
    /// OSC 1337, only when it is a real local directory), then the deepest
    /// living descendant's OS-level cwd as a last resort.
    pub fn detected_cwd(&self) -> Option<PathBuf> {
        #[cfg(unix)]
        {
            if let Some(p) = self.foreground_cwd() {
                return Some(p);
            }
        }
        if let Some(p) = self.vt.pwd() {
            if p.is_absolute() && p.is_dir() {
                return Some(p);
            }
        }
        let root = self.pty.process_id()?;
        let snap = ProcessSnapshot::capture()?;
        let leaf = snap.deepest_descendant(root).unwrap_or(root);
        process_cwd(leaf)
    }

    /// The cwd of the process group currently controlling this pane's PTY —
    /// the foreground job (the shell at a prompt, or the running command).
    /// Unlike "deepest descendant", a lingering background process does not
    /// hijack the reported location.
    #[cfg(unix)]
    fn foreground_cwd(&self) -> Option<PathBuf> {
        let pgid = foreground_pgid(self.pty.raw_fd(), self.pty.process_id())?;
        process_cwd(pgid)
    }

    /// Milliseconds since the pane last produced output. Debug/diagnostics.
    #[allow(dead_code)]
    pub fn last_output_age(&self) -> Duration {
        self.last_output.elapsed()
    }

    /// Last `max_chars` of stripped output text. Debug/diagnostics.
    #[allow(dead_code)]
    pub fn recent_text_tail(&self, max_chars: usize) -> String {
        let active = &self.recent_text[self.recent_text_start..];
        let start = active.len().saturating_sub(max_chars);
        String::from_utf8_lossy(&active[start..]).into_owned()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        self.vt.resize(cols, rows);
        self.dirty = true;
        self.full_redraw = true;
        let _ = self.pty.resize(cols, rows);
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
    #[allow(dead_code)]
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
                    c.set_diff_option(CellDiffOption::None);
                }
            }
        }

        // Plain-text row content for the written rows, rebuilt from the cells
        // as they're drawn. While a link modifier is held, URLs detected in it
        // are underlined so plain-text links (e.g. `next dev` output) read as
        // clickable, the same way a normal terminal highlights them.
        let mut row_texts: HashMap<usize, String> = HashMap::new();

        self.vt.for_each_cell(|row, col, rc, selected, row_dirty| {
            if !full && !row_dirty && !dirty.contains(&row) {
                return;
            }
            if row >= ah as usize || col >= aw as usize {
                return;
            }
            if self.link_mods {
                row_texts.entry(row).or_default().push_str(rc.text);
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
            if self.link_mods && rc.hyperlink {
                // OSC 8 hyperlinks are underlined while a link modifier is held
                // so they read as clickable.
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
            // Effective glyph width. `unicode-width` covers the common case
            // (CJK, EAW=Wide emoji). The emulator's own grapheme width handles
            // the rest with its exact text-layout rules: VS16 forces emoji
            // presentation (wide), VS15 forces text presentation (narrow),
            // and ZWJ/skin-tone/regional-indicator sequences widen the
            // cluster. This mirrors what the client terminal draws, so narrow
            // symbols like arrows (↑ ↓ ← →) stay at width 1 and starship's
            // `[(↑)]` keeps its closing bracket.
            let w = rc.text.cell_width();
            let wide = w >= 2 || (!rc.text.is_empty() && vt::grapheme_width(rc.text) == 2);
            if rc.text.is_empty() {
                bcell.set_char(' ').set_fg(cell_fg).set_bg(cell_bg);
                bcell.modifier = mods;
            } else if wide && col + 1 >= aw as usize {
                // A wide character at the last column: its right half would
                // hang off the grid and spill over the client's pane border
                // (the `│`). Clip it to a blank cell with its style, the same
                // way a real terminal clips a wide glyph at the right edge.
                bcell.set_char(' ').set_fg(cell_fg).set_bg(cell_bg);
                bcell.modifier = mods;
            } else {
                // Write the full grapheme cluster (not just its first
                // codepoint) so multi-codepoint emoji survive: flags, skin
                // tones, and ZWJ sequences like family emoji.
                bcell.set_symbol(rc.text).set_fg(cell_fg).set_bg(cell_bg);
                bcell.modifier = mods;
                // A wide character occupies two columns. Pin its reported width
                // to 2 so the wire carries `cell_width = 2` even when
                // `unicode-width` under-counts it (the `So` emoji above), and
                // mark the continuation cell as `skip` so `from_ratatui`
                // serializes it with `cell_width = 0` and the client skips it.
                // Otherwise the continuation is sent as a normal space and the
                // client overwrites the wide character's right half, leaving
                // visual residue (ghost letters after emoji).
                if wide && w < 2 {
                    bcell.set_diff_option(CellDiffOption::ForcedWidth(
                        std::num::NonZeroU16::new(2).expect("2 is non-zero"),
                    ));
                }
                if wide && col + 1 < aw as usize {
                    let next_xy = (area_x + col as u16 + 1, area_y + row as u16);
                    if let Some(next) = cache.cell_mut(next_xy) {
                        if next.symbol().chars().all(|c| c.is_whitespace()) {
                            next.set_char(' ').set_fg(cell_fg).set_bg(cell_bg);
                            next.set_diff_option(CellDiffOption::Skip);
                        }
                    }
                }
            }
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

        // Underline plain-text URLs on the rows written this frame while a link
        // modifier is held (OSC 8 hyperlinks were underlined in the cell pass
        // above). URLs are ASCII, so char offsets map 1:1 onto cell columns
        // within the row.
        if self.link_mods {
            for (row, line) in row_texts {
                for (s, e, _) in find_urls(&line) {
                    let c0 = line[..s].chars().count() as u16;
                    let c1 = line[..e].chars().count() as u16;
                    for c in c0..c1 {
                        let x = area_x + c;
                        let y = area_y + row as u16;
                        if let Some(bcell) = cache.cell_mut((x, y)) {
                            bcell.modifier |= Modifier::UNDERLINED;
                        }
                    }
                }
            }
        }

        let dirty_vec: Vec<usize> = dirty.iter().copied().collect();
        self.vt.clear_dirty(&dirty_vec);
        self.dirty = false;

        if self.is_ai_cli() {
            Some(self.compute_agent_status())
        } else {
            None
        }
    }

    /// Agent lifecycle state, derived from a snapshot of the terminal buffer
    /// (see [`crate::daemon::agents`]): Blocked/Working win via distinctive markers,
    /// Idle is the fallback.
    pub fn agent_status(&self) -> AgentStatus {
        self.compute_agent_status()
    }

    fn compute_agent_status(&self) -> AgentStatus {
        if self.dead {
            return AgentStatus::Idle;
        }
        crate::daemon::agents::detect(&crate::daemon::agents::Snapshot::capture(&self.vt))
    }

    /// Install the terminal's active selection from two viewport coordinates.
    #[allow(dead_code)]
    pub fn set_selection(&mut self, start: (u16, u16), end: (u16, u16)) -> bool {
        let ok = self.vt.set_selection(start, end);
        if ok {
            self.dirty = true;
            self.full_redraw = true;
        }
        ok
    }

    /// Clear the terminal's active selection.
    #[allow(dead_code)]
    pub fn clear_selection(&mut self) {
        self.vt.clear_selection();
        self.dirty = true;
        self.full_redraw = true;
    }

    /// Extract the text between two viewport coordinates as plain text,
    /// building a fresh selection at extraction time (unwrap + trim).
    #[allow(dead_code)]
    pub fn selection_text(&mut self, start: (u16, u16), end: (u16, u16)) -> Option<String> {
        self.vt.selection_text(start, end)
    }

    /// The clickable link at a pane-relative viewport position: an OSC 8
    /// hyperlink URI, else a plain-text `scheme://` URL on the row (matching a
    /// normal terminal's detection of e.g. `next dev` output).
    #[allow(dead_code)]
    pub fn link_at(&self, col: u16, row: u16) -> Option<String> {
        self.vt.link_at(col, row)
    }

    /// Every clickable link covering viewport row `row`, as column ranges:
    /// OSC 8 hyperlink runs first, then plain-text `scheme://` URLs. Streamed
    /// in `PaneFrame` rows so the client can underline links while a link
    /// modifier is held and open them on click.
    pub fn link_ranges(&self, row: u16) -> Vec<kumo_protocol::LinkRange> {
        let cols = self.vt.cols();
        let mut out: Vec<kumo_protocol::LinkRange> = Vec::new();
        // OSC 8 hyperlinks: contiguous columns sharing one URI form a run.
        let mut run: Option<(u16, String)> = None;
        for col in 0..cols {
            let uri = self.vt.hyperlink_at(col, row);
            match (run.take(), uri) {
                (None, Some(u)) => run = Some((col, u)),
                (Some((s, ru)), Some(u)) if ru == u => run = Some((s, u)),
                (Some((s, ru)), Some(u)) => {
                    out.push(kumo_protocol::LinkRange { start: s, end: col, url: ru });
                    run = Some((col, u));
                }
                (Some((s, ru)), None) => {
                    out.push(kumo_protocol::LinkRange { start: s, end: col, url: ru });
                    run = None;
                }
                (None, None) => {}
            }
        }
        if let Some((s, ru)) = run {
            out.push(kumo_protocol::LinkRange { start: s, end: cols, url: ru });
        }
        // Plain-text URLs on the row, unless an OSC 8 run already covers them.
        let line = self.vt.row_text(row);
        if !line.is_empty() {
            for (s, e, url) in crate::daemon::vt::find_urls(&line) {
                let c0 = line[..s].chars().count() as u16;
                let c1 = line[..e].chars().count() as u16;
                if !out.iter().any(|r| c0 < r.end && c1 > r.start) {
                    out.push(kumo_protocol::LinkRange { start: c0, end: c1, url });
                }
            }
        }
        out
    }

    /// Viewport position of the terminal cursor, relative to the pane origin.
    /// Requires a prior `render` (which refreshes the render state).
    #[allow(dead_code)]
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

    /// The deepest living descendant of `root` (a process with no children),
    /// or `root` itself when it is a leaf. The leaf is where you *are*: a
    /// shell's cwd only moves when a child (editor, `cd`ed tool) forked from
    /// it, so its deepest child is the most current location.
    pub fn deepest_descendant(&self, root: u32) -> Option<u32> {
        let mut best: Option<(usize, u32)> = None;
        let mut stack = vec![(root, 0usize)];
        let mut seen = HashSet::new();
        while let Some((pid, depth)) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if best.map(|(d, _)| depth > d).unwrap_or(true) {
                best = Some((depth, pid));
            }
            if let Some(kids) = self.children.get(&pid) {
                for &k in kids {
                    stack.push((k, depth + 1));
                }
            }
        }
        best.map(|(_, pid)| pid)
    }
}

/// OS-level working directory of `pid`. Linux reads `/proc/<pid>/cwd`; macOS
/// prefers `proc_pidinfo(PROC_PIDVNODEPATHINFO)` (no subprocess spawn) and
/// falls back to `lsof` when that path is not a usable local directory.
/// Other platforms have no supported mechanism.
fn process_cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(target_os = "macos")]
    {
        proc_pidinfo_cwd(pid)
            .filter(|p| p.is_absolute() && p.is_dir())
            .or_else(|| lsof_cwd(pid))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// The foreground process group of the pane's controlling terminal: which
/// process group is currently running in it. Primary: `tcgetpgrp` on the PTY
/// master fd. Fallbacks: `e_tpgid` from `proc_pidinfo` (macOS) and the `tpgid`
/// field of `/proc/<pid>/stat` (Linux).
#[cfg(unix)]
fn foreground_pgid(master_fd: Option<i32>, child_pid: Option<u32>) -> Option<u32> {
    if let Some(fd) = master_fd {
        let pgid = unsafe { libc::tcgetpgrp(fd) };
        if pgid > 0 {
            return Some(pgid as u32);
        }
    }
    let pid = child_pid?;
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let ret = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if ret == size && info.e_tpgid > 0 {
            Some(info.e_tpgid as u32)
        } else {
            None
        }
    }
    #[cfg(target_os = "linux")]
    {
        // `/proc/<pid>/stat` field 8 (tpgid); `comm` may contain spaces, so
        // split after the last `)` and index the fields that follow.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = stat.rsplit_once(')').map(|(_, r)| r)?;
        // After `)`: state(3) ppid(4) pgrp(5) session(6) tty_nr(7) tpgid(8).
        rest.split_whitespace().nth(5).and_then(|f| f.parse().ok())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

/// Read `pvi_cdir.vip_path` via `proc_pidinfo(PROC_PIDVNODEPATHINFO)` — no
/// subprocess. The path can be a partial tail for deeply nested directories,
/// so the caller validates it before trusting it.
#[cfg(target_os = "macos")]
fn proc_pidinfo_cwd(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    if pid == 0 {
        return None;
    }
    let mut pathinfo: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let ret = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut pathinfo as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if ret != size {
        return None;
    }
    let vip_path = unsafe {
        std::slice::from_raw_parts(
            &pathinfo.pvi_cdir.vip_path as *const _ as *const u8,
            libc::MAXPATHLEN as usize,
        )
    };
    let nul = vip_path.iter().position(|&b| b == 0)?;
    if nul == 0 {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&vip_path[..nul])))
}

/// Read the `cwd` file descriptor of `pid` via `lsof` (a guaranteed full
/// path, unlike the possibly-partial `proc_pidinfo` vnode path).
#[cfg(target_os = "macos")]
fn lsof_cwd(pid: u32) -> Option<PathBuf> {
    let out = std::process::Command::new("lsof")
        .arg("-a")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-d")
        .arg("cwd")
        .arg("-Fn")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    for line in out.stdout.split(|&b| b == b'\n') {
        let line = String::from_utf8_lossy(line);
        if let Some(path) = line.strip_prefix('n') {
            if path.starts_with('/') {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::sync::mpsc;

    #[test]
    fn grapheme_width_matches_terminal_layout_rules() {
        // Wide via East Asian Width.
        assert_eq!(vt::grapheme_width("🚀"), 2);
        assert_eq!(vt::grapheme_width("中"), 2);
        // VS16 forces emoji presentation (wide) on a valid emoji base like ❤.
        assert_eq!(vt::grapheme_width("❤\u{FE0F}"), 2);
        // VS16 after a non-emoji base (↑) is ignored: starship's `[(↑)]`
        // keeps its closing bracket with or without the selector.
        assert_eq!(vt::grapheme_width("↑\u{FE0F}"), 1);
        // VS15 forces text presentation (narrow).
        assert_eq!(vt::grapheme_width("★\u{FE0E}"), 1);
        // ZWJ family sequence stays a single 2-column cluster.
        assert_eq!(vt::grapheme_width("👨\u{200D}👩\u{200D}👧"), 2);
        // Regional-indicator flag pair.
        assert_eq!(vt::grapheme_width("\u{1F1EA}\u{1F1F8}"), 2);
        // Lone variation selector is zero-width.
        assert_eq!(vt::grapheme_width("\u{FE0F}"), 0);
        // Plain text symbols and arrows stay width 1.
        assert_eq!(vt::grapheme_width("↑"), 1);
        assert_eq!(vt::grapheme_width("⌘"), 1);
        assert_eq!(vt::grapheme_width("❤"), 1);
        // Per-codepoint helper agrees for single chars.
        assert_eq!(vt::codepoint_width('中'), 2);
        assert_eq!(vt::codepoint_width('a'), 1);
    }

    fn test_theme() -> kumo_core::theme::OwnedTheme {
        kumo_core::theme::OwnedTheme::from(kumo_core::theme::THEMES[kumo_core::theme::DEFAULT_THEME_IDX])
    }

    fn test_pane(is_ai: bool) -> Pane {
        let (tx, _rx) = mpsc::channel();
        Pane::spawn(
            1,
            1,
            "/bin/sh".into(),
            Some(("/usr/bin/true".into(), Vec::new())),
            // A stable cwd: `None` would fall back to $HOME, which parallel
            // config/app tests point at a scratch dir they delete mid-run,
            // racing this spawn into an ENOENT.
            Some(std::env::temp_dir()),
            120,
            40,
            is_ai,
            tx,
            &test_theme(),
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
        // Position the footer hint at the bottom of the 40-row screen.
        p.feed(b"\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n");
        p.feed(b"working on it - esc interrupt");
        render_pane(&mut p);
assert_eq!(p.agent_status(), AgentStatus::Working);
    }

    #[test]
    fn idle_when_working_marker_is_older_transcript_not_footer() {
        // A frozen "esc interrupt" from an earlier turn may remain in the
        // scrolled transcript; only the pinned prompt footer counts.
        let mut p = test_pane(true);
        p.feed(b"previous turn output - esc interrupt\n");
        p.feed(b"\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n");
        p.feed(b"Ask anything... \"\"");
        p.last_output = Instant::now() - Duration::from_secs(10);
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Idle);
    }

    #[test]
    fn working_when_footer_shows_interrupt_hint() {
        let mut p = test_pane(true);
        p.feed(b"some transcript text\n");
        p.feed(b"\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n");
        p.feed(b"esc interrupt");
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
    fn idle_when_transcript_contains_generic_prompt_text() {
        // Generic approval text in the conversation transcript must NOT flag
        // the agent as blocked; only real dialogs do.
        let mut p = test_pane(true);
        p.feed(b"the assistant asked: Do you want to proceed? (y/n)\n");
        p.last_output = Instant::now() - Duration::from_secs(10);
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Idle);
    }

    #[test]
    fn blocked_on_opencode_question_dialog() {
        // opencode's question dialog (QuestionPrompt) footer: esc dismiss must
        // be paired with an enter action and a navigation hint to count.
        let mut p = test_pane(true);
        p.feed(b"\xe2\x87\x86 tab   \xe2\x86\x91\xe2\x86\x93 select\n");
        p.feed(b"enter submit   esc dismiss\n");
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Blocked);
    }

    #[test]
    fn idle_when_esc_dismiss_without_question_footer() {
        // opencode's idle prompt must stay Idle even if "esc dismiss"-like
        // text lingers without the question dialog's enter/navigation hints.
        let mut p = test_pane(true);
        p.feed(b"Ask anything... \"\"\n");
        p.feed(b"esc dismiss\n");
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Idle);
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
        p.feed(b"\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n");
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
    fn claude_working_when_osc_title_has_spinner() {
        // Claude Code paints its live state in the OSC window title: a braille
        // spinner while a task runs, `✳ ` when idle.
        let mut p = test_pane(true);
        p.feed("\x1b]0;\u{280b} Fixing the bug\x07".as_bytes());
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Working);
    }

    #[test]
    fn claude_idle_when_osc_title_has_idle_marker() {
        let mut p = test_pane(true);
        p.feed("\x1b]0;\u{2733} ~/proj\x07".as_bytes());
        p.feed(b"\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n");
        p.feed("\u{276f} ".as_bytes());
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Idle);
    }

    #[test]
    fn claude_blocked_on_approval_form() {
        // Generic permission prompt: "do you want to proceed?" + "esc to
        // cancel" + numbered yes/no options below the last horizontal rule.
        let mut p = test_pane(true);
        p.feed(b"Claude wants to run a command\n");
        p.feed("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n".as_bytes());
        p.feed(b"Do you want to proceed?\n");
        p.feed(b"  1. yes\n");
        p.feed(b"  2. no\n");
        p.feed(b"  esc to cancel\n");
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Blocked);
    }

    #[test]
    fn claude_blocked_on_live_form() {
        // Live form: "esc to cancel" + "enter to confirm" (dynamic workflow,
        // snippet save, etc.).
        let mut p = test_pane(true);
        p.feed("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n".as_bytes());
        p.feed(b"Run a dynamic workflow?\n");
        p.feed(b"  enter to confirm\n");
        p.feed(b"  esc to cancel\n");
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Blocked);
    }

    #[test]
    fn claude_blocked_on_bash_approval() {
        let mut p = test_pane(true);
        p.feed(b"Do you want to proceed?\n");
        p.feed(b"  bash(rm -rf build)\n");
        p.feed(b"  1. yes\n");
        p.feed(b"  2. no\n");
        p.feed(b"  esc to cancel\n");
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Blocked);
    }

    #[test]
    fn claude_idle_on_prompt_box() {
        // A bare `❯` prompt means idle, even with generic question text above
        // in the transcript.
        let mut p = test_pane(true);
        p.feed(b"assistant: do you want to proceed? (y/n)\n");
        p.feed(b"\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n");
        p.feed("\u{276f} ".as_bytes());
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Idle);
    }

    #[test]
    fn claude_not_blocked_without_form_chrome() {
        // "esc to cancel" needs the matching enter action; a select list
        // without a navigation hint stays idle.
        let mut p = test_pane(true);
        p.feed("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n".as_bytes());
        p.feed(b"enter to select\n");
        p.feed(b"esc to cancel\n");
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Idle);
    }

    #[test]
    fn claude_working_on_btw_overlay() {
        let mut p = test_pane(true);
        p.feed(b"/btw reasoning about the bug\n");
        p.feed(b"  esc to close\n");
        render_pane(&mut p);
        assert_eq!(p.agent_status(), AgentStatus::Working);
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
    fn opencode_raw_output_renders_without_losing_rows() {
        let raw = match std::fs::read("/tmp/oc_msg.raw") {
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
    fn vim_alt_screen_scroll_down_clears_stale_cells() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);

        // Enter alternate screen, set a scroll region over rows 1..39.
        p.feed(b"\x1b[?1049h\x1b[1;39r");

        // Fill the scroll region with distinct per-row content.
        let mut fill = String::new();
        for i in 0..39 {
            fill.push_str(&format!("\x1b[{};1Hrow{i:02}", i + 1));
        }
        p.feed(fill.as_bytes());
        p.render_dirty(area, true, &mut buf);
        assert!(pane_text(&buf).contains("row00"), "initial fill missing");

        // Scroll down (DL) then back up (IL) like a fast mouse scroll, with a
        // render after each burst, checking no stale cells survive anywhere.
        let mut scrolled: Option<usize> = None;
        for step in 0..14 {
            if step < 7 {
                p.feed(b"\x1b[1;39r\x1b[1;1H\x1b[3M\x1b[1;40r");
            } else {
                p.feed(b"\x1b[1;39r\x1b[1;1H\x1b[3L\x1b[1;40r");
            }
            p.render_dirty(area, true, &mut buf);
            scrolled = Some(step);
        }
        let _ = scrolled;

        // After scrolling down 7 (rows 21..38 shifted to 0..17) and back up 7,
        // the viewport should show rows 0..38 again with NO stale cells: every
        // cell must match the row content expected at its position, or be blank.
        for y in 0..39 {
            let mut line = String::new();
            for x in 0..120 {
                if let Some(cell) = buf.cell((x, y)) {
                    line.push_str(cell.symbol());
                }
            }
            let line = line.trim_end();
            if !line.is_empty() {
                let expect = format!("row{y:02}");
                assert!(
                    line.starts_with(&expect) || line.contains("row"),
                    "stale content at row {y}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn rapid_scrolls_leave_no_stale_cells() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);

        // Enter alt screen, fill a 39-row region with distinct per-row content.
        p.feed(b"\x1b[?1049h\x1b[1;39r");
        let mut fill = String::new();
        for i in 0..39 {
            fill.push_str(&format!("\x1b[{};1Hrow{i:03}", i + 1));
        }
        p.feed(fill.as_bytes());
        p.render_dirty(area, true, &mut buf);
        assert!(pane_text(&buf).contains("row000"));

        // 30 rapid DL-3 scrolls in one burst (faster than any real scroll), then
        // a single render. Every visible cell must be blank or a shifted row.
        let mut burst = Vec::new();
        for _ in 0..30 {
            burst.extend_from_slice(b"\x1b[1;39r\x1b[1;1H\x1b[3M\x1b[1;40r");
        }
        p.feed(&burst);
        p.render_dirty(area, true, &mut buf);

        for y in 0..39 {
            let mut line = String::new();
            for x in 0..120 {
                if let Some(cell) = buf.cell((x, y)) {
                    line.push_str(cell.symbol());
                }
            }
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            // Any visible content must be a valid "rowNNN" (shifted up), never
            // a partial/stale fragment of one.
            let valid = (0..39).any(|i| line.starts_with(&format!("row{i:03}")));
            assert!(valid, "stale content at row {y}: {line:?}");
        }
    }

    #[test]
    fn nvim_scroll_with_emoji_leaves_no_stale_letters() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 183, 45);
        let mut buf = Buffer::empty(area);

        // Enter alt screen, set nvim's scroll region (rows 2..41).
        p.feed(b"\x1b[?1049h\x1b[2;41r");

        // Fill rows 2..41 with distinct lines: an emoji followed by a letter
        // and a row number, mimicking the real changelog content that produced
        // ghost letters like 'T'/'i'/'h'.
        let mut fill = String::new();
        for i in 2..=41 {
            let emoji = match i % 3 {
                0 => "\u{2705}T",
                1 => "\u{1f527}i",
                _ => "\u{1f9e9}h",
            };
            fill.push_str(&format!("\x1b[{i};1H{emoji} line {i:02}"));
        }
        p.feed(fill.as_bytes());
        p.render_dirty(area, true, &mut buf);
        assert!(pane_text(&buf).contains("line 05"), "initial fill missing");

        // Scroll down 12 lines via nvim's DL-3 scroll, feeding each scroll in
        // small chunks with a render between, like the real PTY delivers it.
        for _ in 0..4 {
            p.feed(b"\x1b[2;41r\x1b[2;1H\x1b[3M\x1b[r");
            p.render_dirty(area, true, &mut buf);
        }

        // Every non-blank row must be a recognizable "line NN" content — never
        // a lone orphaned letter.
        for y in 0..buf.area.height as usize {
            let line: String = (0..buf.area.width)
                .map(|x| buf.cell((x, y as u16)).map(|c| c.symbol().to_string()).unwrap_or_default())
                .collect();
            let meaningful: String = line.trim().to_string();
            if meaningful.is_empty() {
                continue;
            }
            if !meaningful.contains("line ") {
                panic!("stale content at row {y}: {meaningful:?}");
            }
        }
    }

    #[test]
    fn multi_codepoint_emoji_survive_rendering_whole() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);

        // One grapheme per row: a flag (two regional indicators), a family
        // (ZWJ sequence), and a thumbs-up with a skin tone modifier. Each
        // occupies a single terminal cell but is multiple codepoints, so
        // truncating to the first `char` would break them.
        let rows = [
            ("\u{1f1ea}\u{1f1f8}", "flag-es"),
            ("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}", "family"),
            ("\u{1f44d}\u{1f3fb}", "thumbs-skin"),
        ];
        let mut feed = String::new();
        for (i, (emoji, label)) in rows.iter().enumerate() {
            feed.push_str(&format!("\x1b[{};1H{emoji} {label}", i + 1));
        }
        p.feed(feed.as_bytes());
        p.render_dirty(area, true, &mut buf);

        for (y, (emoji, label)) in rows.iter().enumerate() {
            // The full grapheme must land in the buffer as a single cell,
            // not truncated to its first codepoint.
            let cell = buf.cell((0, y as u16)).unwrap();
            assert_eq!(cell.symbol(), *emoji, "row {y} truncated the grapheme");
            let row_text: String = (0..buf.area.width)
                .map(|x| buf.cell((x, y as u16)).map(|c| c.symbol().to_string()).unwrap_or_default())
                .collect();
            assert!(
                row_text.contains(label),
                "row {y} lost the label: {row_text:?}"
            );
        }
    }

    #[test]
    fn select_text_in_opencode() {
        let raw = match std::fs::read("/tmp/oc_msg.raw") {
            Ok(d) => d,
            Err(_) => return, // captured fixture not present
        };
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

    #[test]
    fn deepest_descendant_returns_leaf() {
        let snap = ProcessSnapshot {
            children: HashMap::from([(1, vec![2, 3]), (2, vec![4]), (4, vec![5])]),
            names: HashMap::new(),
        };
        assert_eq!(snap.deepest_descendant(1), Some(5), "deepest child wins");
        assert_eq!(snap.deepest_descendant(3), Some(3), "a leaf is its own deepest descendant");
        assert_eq!(snap.deepest_descendant(2), Some(5));
    }

    #[test]
    fn process_cwd_reads_current_process() {
        let cwd = process_cwd(std::process::id()).expect("our own cwd must be readable");
        assert_eq!(cwd, std::env::current_dir().unwrap(), "process_cwd(self) must match current_dir");
    }

    #[test]
    fn detected_cwd_follows_shell_cd() {
        let dir = std::env::temp_dir().join(format!("kumo-pane-cwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, _rx) = mpsc::channel();
        let mut pane = Pane::spawn(1, 1, "/bin/sh".into(), None, None, 80, 24, false, tx, &test_theme()).unwrap();
        pane.write(format!("cd {}\n", dir.display()).as_bytes());
        std::thread::sleep(Duration::from_millis(600));
        // macOS /var and /tmp resolve to /private; compare canonical paths.
        let canon = |p: &PathBuf| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let cwd = pane.detected_cwd().map(|p| canon(&p));
        assert_eq!(cwd, Some(canon(&dir)), "detected_cwd must follow a shell cd");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detected_cwd_follows_shell_not_background_job() {
        // A lingering background process must not hijack follow-workspace:
        // `sleep 3 &` then `cd` — the foreground process group is still the
        // shell, so the reported cwd is the shell's, not the sleep's.
        let root = std::env::temp_dir().join(format!("kumo-pane-bg-{}", std::process::id()));
        let bg = root.join("bg");
        let cwd_dir = root.join("cwd");
        for d in [&bg, &cwd_dir] {
            let _ = std::fs::remove_dir_all(d);
            std::fs::create_dir_all(d).unwrap();
        }
        let (tx, _rx) = mpsc::channel();
        let mut pane = Pane::spawn(1, 1, "/bin/sh".into(), None, None, 80, 24, false, tx, &test_theme()).unwrap();
        pane.write(format!("cd {}\nsleep 3 &\ncd {}\n", bg.display(), cwd_dir.display()).as_bytes());
        std::thread::sleep(Duration::from_millis(900));
        let canon = |p: &PathBuf| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let cwd = pane.detected_cwd().map(|p| canon(&p));
        assert_eq!(
            cwd,
            Some(canon(&cwd_dir)),
            "follow must report the shell's cwd, not the background job's"
        );
        for d in [&bg, &cwd_dir] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn opencode_pane_reports_mouse_tracking() {
        let raw = match std::fs::read("/tmp/oc_msg.raw") {
            Ok(d) => d,
            Err(_) => return, // captured fixture not present
        };
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        for chunk in raw.chunks(256) {
            p.feed(chunk);
            p.render_dirty(area, true, &mut buf);
        }
        assert!(p.has_mouse_reporting(), "opencode should enable mouse tracking");
    }

    #[test]
    fn resume_restores_mouse_tracking() {
        // A pane running a full-screen TUI has mouse reporting on; the live
        // app keeps it app-side across a daemon restart, so a resumed pane
        // must re-learn it from the snapshot or kumo would grab the mouse.
        let mut p = test_pane(false);
        p.feed(b"\x1b[?1000h");
        assert!(p.has_mouse_reporting(), "mode 1000 enables mouse tracking");

        let fd = p.pty.raw_fd().expect("spawned pty exposes its master fd");
        let child_pid = p.pty.process_id();
        // Simulate exec-inheritance: forget the handle so its fd stays open
        // for `Pane::resume` to adopt (mirrors `resumed_pty_keeps_live_shell_io`).
        std::mem::forget(p);

        let (tx, _rx) = mpsc::channel();
        let mut resumed = Pane::resume(
            1,
            2,
            "/bin/sh".into(),
            None,
            PathBuf::from("/"),
            120,
            40,
            true,
            fd,
            child_pid.map(|c| c as i32),
            true,
            None,
            tx,
            &test_theme(),
        )
        .unwrap();
        assert!(
            resumed.has_mouse_reporting(),
            "mouse tracking must be restored on a resumed pane"
        );
        resumed.pty.kill();
    }

    #[test]
    fn resume_without_mouse_tracking_stays_off() {
        // A pane that had mouse reporting off (e.g. a plain shell) must not
        // start forwarding the mouse after a resume.
        let p = test_pane(false);
        let fd = p.pty.raw_fd().expect("spawned pty exposes its master fd");
        let child_pid = p.pty.process_id();
        std::mem::forget(p);

        let (tx, _rx) = mpsc::channel();
        let mut resumed = Pane::resume(
            1,
            2,
            "/bin/sh".into(),
            None,
            PathBuf::from("/"),
            120,
            40,
            false,
            fd,
            child_pid.map(|c| c as i32),
            false,
            None,
            tx,
            &test_theme(),
        )
        .unwrap();
        assert!(!resumed.has_mouse_reporting(), "mouse tracking must stay off");
        resumed.pty.kill();
    }

    /// Poll `fd` and read/discard until no data arrives for `quiet`.
    fn drain_until_quiet(fd: i32, quiet: Duration) {
        let mut buf = [0u8; 4096];
        let deadline = Instant::now() + quiet;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let rc = unsafe { libc::poll(&mut pfd, 1, remaining.as_millis() as i32) };
            if rc <= 0 || pfd.revents & libc::POLLIN == 0 {
                return;
            }
            let n =
                unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                return;
            }
        }
    }

    #[test]
    fn resumed_pane_nudges_process_to_repaint() {
        // Restart-in-place resumes a pane at the same size the inherited PTY is
        // already at, into a fresh (blank) emulator. Pre-fix, no later layout
        // resize changed the size, so the kernel never fired SIGWINCH and the
        // process never repainted: the pane stayed empty (only the session
        // active at restart came back, by the accident of the daemon's initial
        // small render forcing a real resize). The resume must nudge the
        // process by briefly changing the size so it redraws into the fresh
        // emulator.
        use std::time::{Duration, Instant};

        let spec = PtySpec {
            shell: "/bin/sh".into(),
            program: None,
            cwd: Some(std::env::temp_dir()),
            cols: 80,
            rows: 24,
        };
        let mut pty = Pty::spawn(&spec).unwrap();
        pty.write(b"PS1='KUMO_NUDGE> '\r\necho KUMO_RESUME_NUDGE\r\n").unwrap();

        // Wait until the marker is echoed, then drain until the PTY is quiet,
        // so any output after resume is a repaint from the nudge (not stale).
        let fd = pty.raw_fd().expect("spawned pty exposes its master fd");
        let mut all = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !all.windows(b"KUMO_RESUME_NUDGE".len()).any(|w| w == b"KUMO_RESUME_NUDGE") {
            assert!(Instant::now() < deadline, "marker never echoed");
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            if unsafe { libc::poll(&mut pfd, 1, 50) } <= 0 || pfd.revents & libc::POLLIN == 0 {
                continue;
            }
            let mut buf = [0u8; 4096];
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            all.extend_from_slice(&buf[..n as usize]);
        }
        assert!(
            all.windows(b"KUMO_RESUME_NUDGE".len()).any(|w| w == b"KUMO_RESUME_NUDGE"),
            "marker never echoed: {:?}",
            String::from_utf8_lossy(&all)
        );
        drain_until_quiet(fd, Duration::from_millis(250));

        // Simulate exec-inheritance: forget the handle so its fd survives.
        let child_pid = pty.process_id();
        std::mem::forget(pty);

        let (tx, rx) = mpsc::channel();
        let mut resumed = Pane::resume(
            1,
            2,
            "/bin/sh".into(),
            None,
            std::env::temp_dir(),
            80,
            24,
            false,
            fd,
            child_pid.map(|c| c as i32),
            false,
            None,
            tx,
            &test_theme(),
        )
        .unwrap();

        // The nudge must make the shell repaint its prompt into the fresh
        // emulator; a same-size resume emits nothing on its own.
        let mut fed = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            while let Ok(ev) = rx.try_recv() {
                // `PtyEvent` has a single variant, so the pattern is exhaustive.
                let PtyEvent::Output { data, .. } = ev;
                fed.extend_from_slice(&data);
                resumed.feed(&data);
            }
            if !fed.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!fed.is_empty(), "resume never nudged the shell to repaint");

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        resumed.render_dirty(Rect::new(0, 0, 80, 24), true, &mut buf);
        let text = pane_text(&buf);
        assert!(
            text.contains("KUMO_NUDGE>") || text.contains("KUMO_RESUME_NUDGE"),
            "repainted prompt missing from the resumed pane: {text:?}"
        );

        // The kernel must be left one column narrower than the emulator, so the
        // app's first layout render (which always resizes a pane once) is a
        // genuine size change — TUIs that skip a same-size round-trip redraw
        // must still repaint then.
        assert_eq!(resumed.pty.cols, 79, "resume must leave the PTY narrower");
        assert_eq!(resumed.pty.rows, 24);

        // Drain the resume-time repaint, then resize back to the real size: the
        // first layout resize must fire a fresh repaint of its own.
        std::thread::sleep(Duration::from_millis(200));
        while let Ok(ev) = rx.try_recv() {
            // `PtyEvent` has a single variant, so the pattern is exhaustive.
            let PtyEvent::Output { data, .. } = ev;
            fed.extend_from_slice(&data);
        }
        let settled = fed.len();
        resumed.resize(80, 24);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            while let Ok(ev) = rx.try_recv() {
                let PtyEvent::Output { data, .. } = ev;
                fed.extend_from_slice(&data);
                resumed.feed(&data);
            }
            if fed.len() > settled {
                break;
            }
            assert!(Instant::now() < deadline, "first layout resize never repainted");
            std::thread::sleep(Duration::from_millis(20));
        }
        resumed.pty.kill();
    }

    #[test]
    fn osc8_hyperlinks_render_underlined() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);

        // Without a link modifier held, no underline is added.
        p.feed(b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07");
        p.render_dirty(area, true, &mut buf);
        assert!(!buf.cell((0, 0)).unwrap().modifier.contains(Modifier::UNDERLINED));

        // While a link modifier is held, every link cell is underlined.
        p.link_mods = true;
        p.render_dirty(area, true, &mut buf);
        for x in 0..4 {
            assert!(
                buf.cell((x, 0)).unwrap().modifier.contains(Modifier::UNDERLINED),
                "link cell {x} not underlined"
            );
        }
        assert!(!buf.cell((5, 0)).unwrap().modifier.contains(Modifier::UNDERLINED));

        // The URI resolves at pane-relative viewport coordinates (OSC 8).
        assert_eq!(p.link_at(0, 0).as_deref(), Some("https://example.com"));
        assert_eq!(p.link_at(5, 0), None);
    }

    #[test]
    fn plain_text_urls_render_underlined_and_clickable() {
        let mut p = test_pane(false);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);

        // `next dev`-style output: the URL is plain text, not an OSC 8 link.
        p.feed(b"- Local: http://localhost:3000\r\n");
        p.link_mods = true;
        p.render_dirty(area, true, &mut buf);

        let row_text: String = (0..buf.area.width)
            .map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()).unwrap_or_default())
            .collect();
        let url_start = row_text.find("http://localhost:3000").unwrap();
        // Every URL cell is underlined...
        for i in 0.."http://localhost:3000".len() {
            assert!(
                buf.cell((url_start as u16 + i as u16, 0)).unwrap().modifier.contains(Modifier::UNDERLINED),
                "url cell {i} not underlined"
            );
        }
        // ...and the character right before the URL is not.
        assert!(!buf.cell((url_start as u16 - 1, 0)).unwrap().modifier.contains(Modifier::UNDERLINED));

        // Without a link modifier held, the URL is not underlined (the app
        // forces a full redraw when the modifier toggles, as `set_link_mods`).
        p.link_mods = false;
        p.dirty = true;
        p.full_redraw = true;
        p.render_dirty(area, true, &mut buf);
        assert!(!buf.cell((url_start as u16, 0)).unwrap().modifier.contains(Modifier::UNDERLINED));

        // The click resolves at the pane-relative column.
        assert_eq!(p.link_at(url_start as u16, 0).as_deref(), Some("http://localhost:3000"));
        assert_eq!(p.link_at(url_start as u16 - 1, 0), None);
    }
}
