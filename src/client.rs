//! Thin TUI client: a "dumb viewport" for the kumo daemon.
//!
//! The daemon owns everything and never renders chrome. This client:
//! 1. attaches (`Command::Attach`) and subscribes to the semantic layout,
//! 2. computes pane geometry from the split tree (ratios) over its terminal,
//! 3. requests per-pane sizes (`PaneResize`) and subscribes to `PaneFrame`s,
//! 4. draws the borders/titles/status itself and forwards keys as commands
//!    (the leader keymap lives here, not in the daemon).

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};

use crate::app::bindings::{self, Action, Chord};
use crate::app::{Dir, Launch};
use crate::protocol::{
    self, ClientKind, Command, DaemonEvent, Layout, LayoutNode, PaneFrame, SplitDir, WireCell,
    WireKeyEvent,
};

/// How the client attaches.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Leader,
}

pub fn run(launch: Launch) -> Result<()> {
    let path = crate::config::ipc_socket_path();
    let mut spawned = false;
    let stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) => match launch {
            Launch::Attach => {
                anyhow::bail!("no kumo daemon is running (start with `kumo` or `kumo new`)")
            }
            _ => {
                spawn_daemon(workspace_for(&launch))?;
                wait_for_daemon(&path)?;
                spawned = true;
                UnixStream::connect(&path)?
            }
        },
    };
    client_loop(stream, &launch, spawned)
}

fn workspace_for(launch: &Launch) -> Option<PathBuf> {
    match launch {
        Launch::New(Some(p)) => Some(p.clone()),
        _ => None,
    }
}

enum Exit {
    Clean,
    Restarting,
}

fn client_loop(mut stream: UnixStream, launch: &Launch, spawned: bool) -> Result<()> {
    loop {
        match client_once(&mut stream, launch, spawned) {
            Ok(Exit::Clean) => return Ok(()),
            Ok(Exit::Restarting) => {
                stream = reconnect()?;
            }
            Err(e) => return Err(e),
        }
    }
}

fn client_once(stream: &mut UnixStream, launch: &Launch, spawned: bool) -> Result<Exit> {
    let (cols, rows) = crossterm::terminal::size()?;
    protocol::write_framed(
        stream,
        &Command::Attach { protocol: protocol::PROTOCOL_VERSION, kind: ClientKind::Terminal, cols, rows },
    )?;
    protocol::write_framed(stream, &Command::SubscribeLayout)?;
    // `kumo new [dir]` against a running daemon: create a fresh session first.
    if !spawned && matches!(launch, Launch::New(_)) {
        let workspace = workspace_for(launch).or_else(|| std::env::current_dir().ok());
        protocol::write_framed(stream, &Command::SessionNew { name: None, workspace })?;
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        EnableBracketedPaste,
        Hide,
        Clear(ClearType::All),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
    )?;

    // Daemon-event reader thread.
    let write_half = stream.try_clone()?;
    let (ev_tx, ev_rx) = mpsc::channel::<DaemonEvent>();
    let reader = std::thread::spawn(move || reader_loop(write_half, ev_tx));

    let leader = crate::config::leader()
        .and_then(|raw| bindings::parse_chord(&raw))
        .unwrap_or(bindings::LEADER);
    let keymap = bindings::build_keymap(&crate::config::keymap_bindings());
    let mut view = View::new(stream.try_clone()?, cols, rows, leader, keymap);

    let result: Result<Exit> = (|| {
        loop {
            while let Ok(ev) = ev_rx.try_recv() {
                match ev {
                    DaemonEvent::Layout { layout } => view.on_layout(&layout),
                    DaemonEvent::PaneFrame { frame } => view.on_pane_frame(&frame),
                    DaemonEvent::Welcome { .. } => view.status = "connected".into(),
                    DaemonEvent::Reply { message } => view.status = message,
                    DaemonEvent::ConfigReloaded { notice } => view.status = notice,
                    DaemonEvent::Restarting => return Ok(Exit::Restarting),
                    DaemonEvent::Detach | DaemonEvent::Shutdown => return Ok(Exit::Clean),
                    _ => {}
                }
            }

            if event::poll(Duration::from_millis(16))? {
                loop {
                    match event::read()? {
                        event::Event::Key(k) => view.on_key(k),
                        event::Event::Paste(text) => view.on_paste(&text),
                        event::Event::Mouse(m) => view.on_mouse(m),
                        event::Event::Resize(w, h) => view.on_resize(w, h),
                        _ => {}
                    }
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }

            if view.dirty {
                view.draw(&mut stdout)?;
            }
        }
    })();

    let _ = reader.join();
    let _ = execute!(
        stdout,
        Show,
        crossterm::event::DisableMouseCapture,
        DisableBracketedPaste,
        PopKeyboardEnhancementFlags,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    let _ = stdout.flush();
    result
}

/// Read `DaemonEvent`s from the socket and push them to the main loop.
fn reader_loop(mut stream: UnixStream, tx: mpsc::Sender<DaemonEvent>) {
    loop {
        match protocol::read_framed::<DaemonEvent>(&mut stream) {
            Ok(ev) => {
                if tx.send(ev).is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// A retained per-pane grid assembled from `PaneFrame`s.
#[derive(Default)]
struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<WireCell>>,
}

impl Grid {
    fn apply(&mut self, frame: &PaneFrame) {
        if frame.full {
            self.cols = frame.cols as usize;
            self.rows = frame.rows as usize;
            self.cells = vec![vec![blank_cell(); self.cols]; self.rows];
        } else if self.rows != frame.rows as usize || self.cols != frame.cols as usize {
            return;
        }
        for patch in &frame.rows_dirty {
            let Some(row) = self.cells.get_mut(patch.row as usize) else { continue };
            for (x, cell) in patch.cells.iter().enumerate() {
                if let Some(slot) = row.get_mut(x) {
                    *slot = cell.clone();
                }
            }
        }
    }
}

fn blank_cell() -> WireCell {
    WireCell {
        text: String::new(),
        fg: None,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
        inverse: false,
        faint: false,
        cell_width: 1,
    }
}

type Rect = (u16, u16, u16, u16); // x, y, w, h

/// Client-side view: layout, per-pane grids, geometry, and the leader keymap.
struct View {
    out: UnixStream,
    cols: u16,
    rows: u16,
    leader: Chord,
    keymap: Vec<bindings::Binding>,
    mode: Mode,
    layout: Option<Layout>,
    grids: HashMap<u64, Grid>,
    rects: Vec<(u64, Rect)>,
    subscribed: HashSet<u64>,
    sent_sizes: HashMap<u64, (u16, u16)>,
    status: String,
    dirty: bool,
}

impl View {
    fn new(out: UnixStream, cols: u16, rows: u16, leader: Chord, keymap: Vec<bindings::Binding>) -> Self {
        Self {
            out,
            cols,
            rows,
            leader,
            keymap,
            mode: Mode::Normal,
            layout: None,
            grids: HashMap::new(),
            rects: Vec::new(),
            subscribed: HashSet::new(),
            sent_sizes: HashMap::new(),
            status: "connecting…".into(),
            dirty: true,
        }
    }

    fn send(&mut self, cmd: &Command) {
        let _ = protocol::write_framed(&mut self.out, cmd);
    }

    fn active_session(&self) -> Option<&protocol::SessionLayout> {
        let layout = self.layout.as_ref()?;
        let name = layout.active.as_deref()?;
        layout.sessions.iter().find(|s| s.name == name)
    }

    /// Recompute geometry from the semantic tree over the terminal, request
    /// pane sizes, and (re)subscribe to pane streams.
    fn on_layout(&mut self, layout: &Layout) {
        self.layout = Some(layout.clone());
        let mut want: HashSet<u64> = HashSet::new();
        if let Some(session) = layout.sessions.iter().find(|s| Some(&s.name) == layout.active.as_ref()) {
            // Pane area: reserve the last row for the status bar.
            let area = (0u16, 0u16, self.cols, self.rows.saturating_sub(1));
            let rects = if session.zoom {
                vec![(session.focus, area)]
            } else if let Some(root) = &session.root {
                compute_rects(root, area)
            } else {
                Vec::new()
            };
            self.rects = rects;
            for (pid, (_x, _y, w, h)) in self.rects.clone() {
                want.insert(pid);
                let inner_w = w.saturating_sub(2);
                let inner_h = h.saturating_sub(2);
                if self.sent_sizes.get(&pid) != Some(&(inner_w, inner_h)) {
                    self.sent_sizes.insert(pid, (inner_w, inner_h));
                    self.send(&Command::PaneResize { pane_id: pid, cols: inner_w, rows: inner_h });
                }
                if self.subscribed.insert(pid) {
                    self.send(&Command::SubscribePane { pane_id: pid });
                }
            }
        }
        // Unsubscribe panes that left the layout.
        for pid in self.subscribed.difference(&want).copied().collect::<Vec<_>>() {
            self.subscribed.remove(&pid);
            self.send(&Command::UnsubscribePane { pane_id: pid });
            self.grids.remove(&pid);
        }
        self.dirty = true;
    }

    fn on_pane_frame(&mut self, frame: &PaneFrame) {
        let grid = self.grids.entry(frame.pane_id).or_default();
        grid.apply(frame);
        self.dirty = true;
    }

    fn on_resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.sent_sizes.clear();
        if let Some(layout) = self.layout.clone() {
            self.on_layout(&layout);
        }
        self.dirty = true;
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    fn on_key(&mut self, key: event::KeyEvent) {
        if self.mode == Mode::Leader {
            if key.code == event::KeyCode::Esc || self.leader.is_leader(key) {
                self.mode = Mode::Normal;
                self.dirty = true;
                return;
            }
            self.leader_command(key);
            return;
        }
        if self.leader.is_leader(key) {
            self.mode = Mode::Leader;
            self.dirty = true;
            return;
        }
        let wire: WireKeyEvent = key.into();
        self.send(&Command::Input { key: wire });
        // Let the daemon render; no client-side echo needed.
    }

    fn leader_command(&mut self, key: event::KeyEvent) {
        self.mode = Mode::Normal;
        let chord = Chord::new(key.code, key.modifiers);
        let Some(binding) = self.keymap.iter().find(|b| b.key == chord) else {
            self.status = "unknown binding".into();
            self.dirty = true;
            return;
        };
        self.run_action(binding.action);
    }

    fn run_action(&mut self, action: Action) {
        let Some(session_name) = self.active_session().map(|s| s.name.clone()) else {
            self.status = "no active session".into();
            self.dirty = true;
            return;
        };
        let session = session_name.clone();
        match action {
            Action::SplitVertical => {
                self.send(&Command::PaneSplit { session, dir: SplitDir::Vertical, is_ai: false });
            }
            Action::SplitHorizontal => {
                self.send(&Command::PaneSplit { session, dir: SplitDir::Horizontal, is_ai: false });
            }
            Action::SplitAi => {
                self.send(&Command::PaneSplit { session, dir: SplitDir::Vertical, is_ai: true });
            }
            Action::ClosePane => {
                self.send(&Command::PaneClose { session, pane_id: None });
            }
            Action::Zoom => {
                self.send(&Command::SessionZoom { session });
            }
            Action::Focus(dir) => {
                if let Some(pid) = self.pane_toward(dir) {
                    self.send(&Command::PaneFocus { session, pane_id: pid });
                }
            }
            Action::Resize(dir) => {
                let dir = match dir {
                    crate::layout::ResizeDir::Left => protocol::ResizeDir::Left,
                    crate::layout::ResizeDir::Down => protocol::ResizeDir::Down,
                    crate::layout::ResizeDir::Up => protocol::ResizeDir::Up,
                    crate::layout::ResizeDir::Right => protocol::ResizeDir::Right,
                };
                self.send(&Command::PaneResizeRatio { session, dir });
            }
            Action::CyclePane => {
                if let Some(pid) = self.cycle_pane() {
                    self.send(&Command::PaneFocus { session, pane_id: pid });
                }
            }
            Action::SwapPanes => {
                self.send(&Command::PaneSwap { session });
            }
            Action::RotateLayout => {
                self.send(&Command::LayoutRotate { session });
            }
            Action::NextSession | Action::PrevSession => {
                if let Some(name) = self.cycle_session(action == Action::NextSession) {
                    self.send(&Command::SessionFocus { name });
                }
            }
            Action::JumpSession(n) => {
                if let Some(name) = self.session_at(n as usize) {
                    self.send(&Command::SessionFocus { name });
                }
            }
            Action::NewSession => {
                self.send(&Command::SessionNew { name: None, workspace: None });
            }
            Action::Detach => {
                self.send(&Command::Detach);
            }
            _ => {
                // NewWorktree, ShowKeybinds, ToggleSidebar, ShowPaneNumbers:
                // client-side chrome not yet implemented.
                self.status = "not implemented in this client".into();
                self.dirty = true;
            }
        }
    }

    /// The pane whose rect is adjacent to the focused pane in `dir`.
    fn pane_toward(&self, dir: Dir) -> Option<u64> {
        let focus = self.active_session()?.focus;
        let (_, frect) = self.rects.iter().find(|(pid, _)| *pid == focus)?;
        let (fx, fy, fw, fh) = *frect;
        let mut best: Option<(u64, f32)> = None;
        for &(pid, (x, y, w, h)) in &self.rects {
            if pid == focus {
                continue;
            }
            let score = match dir {
                Dir::Left => {
                    if x + w <= fx {
                        Some((fy as i32 - (y + h) as i32).abs().min((y as i32 - (fy + fh) as i32).abs()) as f32 + 0.01)
                    } else {
                        None
                    }
                }
                Dir::Right => {
                    if x >= fx + fw {
                        Some(((fy as i32 - (y + h) as i32).abs().min((y as i32 - (fy + fh) as i32).abs())) as f32 + 0.01)
                    } else {
                        None
                    }
                }
                Dir::Up => {
                    if y + h <= fy {
                        Some((fx as i32 - (x + w) as i32).abs().min((x as i32 - (fx + fw) as i32).abs()) as f32 + 0.01)
                    } else {
                        None
                    }
                }
                Dir::Down => {
                    if y >= fy + fh {
                        Some((fx as i32 - (x + w) as i32).abs().min((x as i32 - (fx + fw) as i32).abs()) as f32 + 0.01)
                    } else {
                        None
                    }
                }
            };
            if let Some(score) = score {
                if best.map(|(_, s)| score < s).unwrap_or(true) {
                    best = Some((pid, score));
                }
            }
        }
        best.map(|(pid, _)| pid)
    }

    fn cycle_pane(&self) -> Option<u64> {
        let session = self.active_session()?;
        let mut ids = Vec::new();
        collect_pane_ids(session.root.as_deref()?, &mut ids);
        let focus = session.focus;
        let idx = ids.iter().position(|i| *i == focus).unwrap_or(usize::MAX);
        ids.get((idx + 1) % ids.len()).copied()
    }

    fn cycle_session(&self, forward: bool) -> Option<String> {
        let layout = self.layout.as_ref()?;
        let names: Vec<&String> = layout.sessions.iter().map(|s| &s.name).collect();
        if names.is_empty() {
            return None;
        }
        let idx = names
            .iter()
            .position(|n| Some(*n) == layout.active.as_ref())
            .unwrap_or(0);
        let next = if forward {
            (idx + 1) % names.len()
        } else {
            (idx + names.len() - 1) % names.len()
        };
        Some(names[next].clone())
    }

    fn session_at(&self, n: usize) -> Option<String> {
        let layout = self.layout.as_ref()?;
        layout.sessions.get(n.saturating_sub(1)).map(|s| s.name.clone())
    }

    fn on_paste(&mut self, text: &str) {
        self.send(&Command::Paste { text: text.to_string() });
    }

    fn on_mouse(&mut self, m: event::MouseEvent) {
        // Pane-relative coordinates for the focused pane, so the daemon can
        // scroll it; the daemon routes to the focused pane.
        let wire: protocol::WireMouseEvent = m.into();
        self.send(&Command::Mouse { event: wire });
    }

    // ------------------------------------------------------------------
    // Rendering (all chrome is drawn client-side)
    // ------------------------------------------------------------------

    fn draw(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        let mut buf = String::with_capacity(64 * 1024);
        buf.push_str("\x1b[2J\x1b[H");
        // Pane content + borders.
        let active = self.active_session().map(|s| s.name.clone());
        for (pid, (x, y, w, h)) in self.rects.clone() {
            draw_pane(&mut buf, pid, x, y, w, h, &self.grids, &self.layout, &active);
        }
        draw_status(&mut buf, self.cols, self.rows, &self.layout, &self.status);
        out.write_all(buf.as_bytes())?;
        out.flush()?;
        self.dirty = false;
        Ok(())
    }
}

fn collect_pane_ids(node: &LayoutNode, out: &mut Vec<u64>) {
    match node {
        LayoutNode::Pane(p) => out.push(p.id),
        LayoutNode::Split { a, b, .. } => {
            collect_pane_ids(a, out);
            collect_pane_ids(b, out);
        }
    }
}

/// Recursively lay a semantic tree out over `area` (in cells).
fn compute_rects(node: &LayoutNode, area: Rect) -> Vec<(u64, Rect)> {
    let (x, y, w, h) = area;
    match node {
        LayoutNode::Pane(p) => vec![(p.id, (x, y, w, h))],
        LayoutNode::Split { dir, ratio, a, b } => {
            let mut out = Vec::new();
            let ratio = ratio.clamp(0.01, 0.99);
            match dir {
                SplitDir::Vertical => {
                    if w <= 1 {
                        return vec![];
                    }
                    let aw = ((w as f32) * ratio).round().clamp(1.0, (w - 1) as f32) as u16;
                    out.extend(compute_rects(a, (x, y, aw, h)));
                    out.extend(compute_rects(b, (x + aw, y, w - aw, h)));
                }
                SplitDir::Horizontal => {
                    if h <= 1 {
                        return vec![];
                    }
                    let ah = ((h as f32) * ratio).round().clamp(1.0, (h - 1) as f32) as u16;
                    out.extend(compute_rects(a, (x, y, w, ah)));
                    out.extend(compute_rects(b, (x, y + ah, w, h - ah)));
                }
            }
            out
        }
    }
}

/// The subset of a cell's styling that decides whether to re-emit SGR codes.
type CellStyle = (Option<u32>, Option<u32>, bool, bool, bool, bool, bool);

/// Draw one pane: border, title, and its content (if a grid has arrived).
#[allow(clippy::too_many_arguments)]
fn draw_pane(
    buf: &mut String,
    pid: u64,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    grids: &HashMap<u64, Grid>,
    layout: &Option<Layout>,
    active: &Option<String>,
) {
    if w < 3 || h < 2 {
        return;
    }
    let title = pane_title(pid, layout, active);
    let title = title.chars().take((w - 2).max(1) as usize).collect::<String>();
    // Top border.
    push_at(buf, x, y, &format!("┌{title}┐"));
    // Side borders.
    for yy in y + 1..y + h - 1 {
        push_at(buf, x, yy, "│");
        push_at(buf, x + w - 1, yy, "│");
    }
    // Bottom border.
    push_at(buf, x, y + h - 1, &format!("└{}┘", "─".repeat((w - 2).max(1) as usize)));
    // Content.
    if let Some(grid) = grids.get(&pid) {
        let inner_w = (w - 2) as usize;
        let inner_h = (h - 2) as usize;
        for row in 0..inner_h {
            if let Some(cells) = grid.cells.get(row) {
                let xo = x + 1;
                let yo = y + 1 + row as u16;
                write_cells(buf, cells, xo, yo, inner_w);
            }
        }
    }
}

fn pane_title(pid: u64, layout: &Option<Layout>, active: &Option<String>) -> String {
    let Some(layout) = layout else { return format!("pane {pid}") };
    for s in &layout.sessions {
        if let Some(title) = find_title(&s.root, pid) {
            let mut t = title.trim().to_string();
            if active.as_ref() == Some(&s.name) && s.focus == pid {
                t.push_str(" *");
            }
            return t;
        }
    }
    format!("pane {pid}")
}

fn find_title(node: &Option<Box<LayoutNode>>, pid: u64) -> Option<String> {
    match node {
        Some(n) => match n.as_ref() {
            LayoutNode::Pane(p) if p.id == pid => Some(p.title.clone()),
            LayoutNode::Split { a, b, .. } => {
                find_title(&Some(a.clone()), pid).or_else(|| find_title(&Some(b.clone()), pid))
            }
            _ => None,
        },
        None => None,
    }
}

fn draw_status(buf: &mut String, cols: u16, rows: u16, layout: &Option<Layout>, status: &str) {
    if rows == 0 {
        return;
    }
    let active = layout.as_ref().and_then(|l| l.active.clone()).unwrap_or_else(|| "-".into());
    let agent_count = layout
        .as_ref()
        .map(|l| l.sessions.iter().flat_map(|s| agent_statuses(&s.root)).count())
        .unwrap_or(0);
    let line = format!(
        " kumo · {active} · {status} · {agent_count} agent(s)"
    );
    let truncated: String = line.chars().take(cols.saturating_sub(1) as usize).collect();
    push_at(buf, 0, rows - 1, &truncated);
}

/// Collect agent statuses from a session's tree (for the status bar count).
fn agent_statuses(node: &Option<Box<LayoutNode>>) -> Vec<u64> {
    match node {
        Some(n) => match n.as_ref() {
            LayoutNode::Pane(p) if p.agent.is_some() => vec![p.id],
            LayoutNode::Split { a, b, .. } => {
                let mut v = agent_statuses(&Some(a.clone()));
                v.extend(agent_statuses(&Some(b.clone())));
                v
            }
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// Move the cursor to (x, y) — both 0-based — and write `text`.
fn push_at(buf: &mut String, x: u16, y: u16, text: &str) {
    buf.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
    buf.push_str(text);
}

/// Write a pane's content row at the given absolute position, clipping to
/// `maxw` cells and skipping wide-char continuation cells.
fn write_cells(buf: &mut String, cells: &[WireCell], x: u16, y: u16, maxw: usize) {
    let mut sgr = String::new();
    let mut text = String::new();
    let mut prev_style: Option<CellStyle> = None;
    for cell in cells.iter().take(maxw) {
        if cell.cell_width == 0 {
            continue;
        }
        let style = (cell.fg, cell.bg, cell.bold, cell.italic, cell.underline, cell.inverse, cell.faint);
        if prev_style != Some(style) {
            push_sgr(&mut sgr, &style);
            prev_style = Some(style);
        }
        text.push_str(if cell.text.trim().is_empty() { " " } else { &cell.text });
    }
    buf.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
    buf.push_str(&sgr);
    buf.push_str(&text);
    buf.push_str("\x1b[0m");
}

fn push_sgr(
    buf: &mut String,
    &(fg, bg, bold, italic, underline, inverse, faint): &(
        Option<u32>,
        Option<u32>,
        bool,
        bool,
        bool,
        bool,
        bool,
    ),
) {
    if bold {
        buf.push_str("\x1b[1m");
    }
    if faint {
        buf.push_str("\x1b[2m");
    }
    if italic {
        buf.push_str("\x1b[3m");
    }
    if underline {
        buf.push_str("\x1b[4m");
    }
    if inverse {
        buf.push_str("\x1b[7m");
    }
    if let Some(c) = fg {
        buf.push_str(&format!("\x1b[38;2;{};{};{}m", (c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff));
    }
    if let Some(c) = bg {
        buf.push_str(&format!("\x1b[48;2;{};{};{}m", (c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff));
    }
}

// ---------------------------------------------------------------------------
// Daemon spawn / reconnect
// ---------------------------------------------------------------------------

/// Launch the daemon as a detached process (own session, no stdio) so it
/// survives the client terminal closing.
fn spawn_daemon(workspace: Option<PathBuf>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("daemon");
    if let Some(ws) = workspace {
        cmd.arg(ws);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn()?;
    Ok(())
}

/// Wait (up to a few seconds) for the freshly spawned daemon to bind its socket.
fn wait_for_daemon(path: &std::path::Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if UnixStream::connect(path).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!("kumo daemon did not start in time")
}

fn reconnect() -> Result<UnixStream> {
    let path = crate::config::ipc_socket_path();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(s) = UnixStream::connect(&path) {
            return Ok(s);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("kumo daemon did not come back after the update restart");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
