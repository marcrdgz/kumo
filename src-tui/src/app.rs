use std::collections::HashMap;
use std::io::Stdout;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use alacritty_terminal::grid::Dimensions;
use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use neomux_core::pty::Pty;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Color as RColor, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::{Frame, Terminal};

use crate::layout::{self, LayoutTree, SplitDir, TreeGeom};
use crate::pane::{sgr_mouse, Pane, PtyEvent};

type Term = Terminal<CrosstermBackend<Stdout>>;

/// herdr-style Catppuccin panel colors.
const PANEL_BG: RColor = RColor::Rgb(0x18, 0x18, 0x25);
const PANEL_SEP: RColor = RColor::Rgb(0x31, 0x32, 0x44);
const PANEL_MUTED: RColor = RColor::Rgb(0x6c, 0x70, 0x86);
const BORDER_IDLE: RColor = RColor::Rgb(0x45, 0x47, 0x5a);

struct Session {
    id: u64,
    name: String,
    tree: LayoutTree,
    zoom: bool,
}

#[derive(PartialEq)]
enum Mode {
    Normal,
    Leader,
}

enum Drag {
    Splitter { split_id: u64 },
}

#[derive(Clone, Copy)]
enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Stable rows of the left sidebar, shared by rendering and mouse hit-testing.
#[derive(Clone)]
enum SidebarRow {
    Header(String),
    Spacer,
    Section(String),
    Session(usize),
    Pane(usize, u64),
}

pub struct App {
    sessions: Vec<Session>,
    active: usize,
    panes: HashMap<u64, Pane>,
    mode: Mode,
    drag: Option<Drag>,
    events_tx: mpsc::Sender<PtyEvent>,
    events_rx: mpsc::Receiver<PtyEvent>,
    shell: String,
    ai: (String, Vec<String>),
    workspace: PathBuf,
    term_size: (u16, u16),
    last_sizes: HashMap<u64, (u16, u16)>,
    sidebar_open: bool,
    sidebar_width: u16,
    quit: bool,
}

pub fn run(terminal: &mut Term, workspace: Option<&str>) -> Result<()> {
    let mut app = App::new(workspace)?;
    while !app.quit {
        while let Ok(ev) = app.events_rx.try_recv() {
            app.on_pty_event(ev);
        }
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                crossterm::event::Event::Key(k) => app.on_key(k)?,
                crossterm::event::Event::Mouse(m) => app.on_mouse(m)?,
                crossterm::event::Event::Resize(w, h) => {
                    app.term_size = (w, h);
                }
                _ => {}
            }
        }
        app.frame(terminal)?;
    }
    Ok(())
}

impl App {
    fn new(workspace: Option<&str>) -> Result<App> {
        let shell = neomux_core::config::default_shell();
        let (ai_prog, ai_args) = neomux_core::config::ai_command();
        let ai_prog = neomux_core::config::resolve_program(&ai_prog);
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
        let workspace = workspace
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or(home);

        let (events_tx, events_rx) = mpsc::channel();
        let mut app = App {
            sessions: Vec::new(),
            active: 0,
            panes: HashMap::new(),
            mode: Mode::Normal,
            drag: None,
            events_tx,
            events_rx,
            shell,
            ai: (ai_prog, ai_args),
            workspace,
            term_size: (80, 24),
            last_sizes: HashMap::new(),
            sidebar_open: true,
            sidebar_width: 26,
            quit: false,
        };
        app.new_session()?;
        Ok(app)
    }

    // ----- lifecycle -----

    fn new_session(&mut self) -> Result<()> {
        let sid = self.next_session_id();
        let name = format!("session-{}", self.sessions.len() + 1);
        let pid = Pty::next_pane_id();
        let (cols, rows) = self.pane_dims();
        let pane = Pane::spawn(
            sid,
            pid,
            self.shell.clone(),
            None,
            Some(self.workspace.clone()),
            cols,
            rows,
            false,
            self.events_tx.clone(),
        )?;
        self.panes.insert(pid, pane);
        self.sessions.push(Session {
            id: sid,
            name,
            tree: LayoutTree::new(pid),
            zoom: false,
        });
        self.active = self.sessions.len() - 1;
        Ok(())
    }

    fn next_session_id(&mut self) -> u64 {
        let max = self.sessions.iter().map(|s| s.id).max().unwrap_or(0);
        max + 1
    }

    fn split_active(&mut self, dir: SplitDir, is_ai: bool) -> Result<()> {
        let focus = self.sessions[self.active].tree.focus;
        let sid = self.sessions[self.active].id;
        let pid = Pty::next_pane_id();
        let (cols, rows) = self.active_pane_dims(focus).unwrap_or(self.pane_dims());
        let (program, shell) = if is_ai {
            (Some((self.ai.0.clone(), self.ai.1.clone())), self.shell.clone())
        } else {
            (None, self.shell.clone())
        };
        let pane = Pane::spawn(
            sid,
            pid,
            shell,
            program,
            Some(self.workspace.clone()),
            cols,
            rows,
            is_ai,
            self.events_tx.clone(),
        )?;
        self.panes.insert(pid, pane);
        if !self.sessions[self.active].tree.split(focus, pid, dir) {
            if let Some(mut p) = self.panes.remove(&pid) {
                p.pty.kill();
            }
        }
        Ok(())
    }

    fn close_focused(&mut self) {
        let focus = self.sessions[self.active].tree.focus;
        self.close_pane(focus);
    }

    fn close_pane(&mut self, pid: u64) {
        if let Some(mut pane) = self.panes.remove(&pid) {
            pane.pty.kill();
        }
        self.last_sizes.remove(&pid);

        let empty = self.sessions[self.active].tree.remove_pane(pid);
        if empty {
            self.sessions.remove(self.active);
            if self.sessions.is_empty() {
                self.quit = true;
                return;
            }
            self.active = self.active.min(self.sessions.len() - 1);
        }
    }

    /// Remove panes whose child process has exited, collapsing the layout.
    fn poll_exits(&mut self) {
        let mut exited: Vec<u64> = Vec::new();
        for (pid, pane) in self.panes.iter_mut() {
            if !pane.dead && matches!(pane.pty.child.try_wait(), Ok(Some(_))) {
                pane.dead = true;
                exited.push(*pid);
            }
        }
        for pid in exited {
            if !self.panes.contains_key(&pid) {
                continue;
            }
            let mut idx = None;
            for (i, s) in self.sessions.iter().enumerate() {
                if s.tree.contains(pid) {
                    idx = Some(i);
                    break;
                }
            }
            if let Some(idx) = idx {
                self.close_pane_from_session(idx, pid);
            }
        }
    }

    fn close_pane_from_session(&mut self, idx: usize, pid: u64) {
        if let Some(mut pane) = self.panes.remove(&pid) {
            pane.pty.kill();
        }
        self.last_sizes.remove(&pid);
        let empty = self.sessions[idx].tree.remove_pane(pid);
        if empty {
            self.sessions.remove(idx);
            if self.sessions.is_empty() {
                self.quit = true;
                return;
            }
            self.active = self.active.saturating_sub(1).min(self.sessions.len() - 1);
        }
    }

    // ----- events -----

    fn on_pty_event(&mut self, ev: PtyEvent) {
        let PtyEvent::Output { pane_id, data } = ev;
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.feed(&data);
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let is_leader = ctrl && key.code == KeyCode::Char('b');

        match self.mode {
            Mode::Normal => {
                if is_leader {
                    self.mode = Mode::Leader;
                    return Ok(());
                }
                let focus = self.sessions[self.active].tree.focus;
                if let Some(pane) = self.panes.get_mut(&focus) {
                    let bytes = crate::keys::encode(key);
                    if !bytes.is_empty() {
                        pane.write(&bytes);
                    }
                }
            }
            Mode::Leader => {
                if is_leader || key.code == KeyCode::Esc {
                    self.mode = Mode::Normal;
                    return Ok(());
                }
                self.leader_command(key)?;
            }
        }
        Ok(())
    }

    fn leader_command(&mut self, key: KeyEvent) -> Result<()> {
        self.mode = Mode::Normal;
        match key.code {
            KeyCode::Char('v') => self.split_active(SplitDir::V, false)?,
            KeyCode::Char('-') => self.split_active(SplitDir::H, false)?,
            KeyCode::Char('c') => self.new_session()?,
            KeyCode::Char('x') => self.close_focused(),
            KeyCode::Char('z') => {
                self.sessions[self.active].zoom = !self.sessions[self.active].zoom;
            }
            KeyCode::Char('h') => self.focus_dir(Dir::Left),
            KeyCode::Char('j') => self.focus_dir(Dir::Down),
            KeyCode::Char('k') => self.focus_dir(Dir::Up),
            KeyCode::Char('l') => self.focus_dir(Dir::Right),
            KeyCode::Char('b') => self.sidebar_open = !self.sidebar_open,
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('n') => self.cycle_session(1),
            KeyCode::Char('p') => self.cycle_session(-1),
            KeyCode::Tab => self.cycle_pane(),
            _ => {}
        }
        Ok(())
    }

    fn cycle_session(&mut self, delta: isize) {
        let n = self.sessions.len();
        if n > 1 {
            self.active = ((self.active as isize + delta).rem_euclid(n as isize)) as usize;
        }
    }

    fn cycle_pane(&mut self) {
        let ids = self.sessions[self.active].tree.pane_ids();
        if ids.len() < 2 {
            return;
        }
        let cur = self.sessions[self.active].tree.focus;
        if let Some(pos) = ids.iter().position(|p| *p == cur) {
            self.sessions[self.active].tree.focus = ids[(pos + 1) % ids.len()];
        }
    }

    fn focus_dir(&mut self, dir: Dir) {
        self.sessions[self.active].zoom = false;
        let geom = self.tree_geom();
        let focus = self.sessions[self.active].tree.focus;
        let cur = match geom.panes.iter().find(|p| p.pane_id == focus) {
            Some(p) => p,
            None => return,
        };
        let best = geom
            .panes
            .iter()
            .filter(|p| p.pane_id != focus)
            .filter(|p| match dir {
                Dir::Left => p.rect.right() <= cur.rect.left(),
                Dir::Right => p.rect.left() >= cur.rect.right(),
                Dir::Up => p.rect.bottom() <= cur.rect.top(),
                Dir::Down => p.rect.top() >= cur.rect.bottom(),
            })
            .min_by(|a, b| {
                let da = (a.rect.right() - a.rect.left()).abs_diff(cur.rect.left())
                    + (a.rect.top() as i32 - cur.rect.top() as i32).unsigned_abs() as u16;
                let db = (b.rect.right() - b.rect.left()).abs_diff(cur.rect.left())
                    + (b.rect.top() as i32 - cur.rect.top() as i32).unsigned_abs() as u16;
                da.cmp(&db)
            });
        if let Some(p) = best {
            self.sessions[self.active].tree.focus = p.pane_id;
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        let x = m.column;
        let y = m.row;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if y == 0 {
                    if let Some(i) = self.tab_at(x) {
                        self.active = i;
                        return Ok(());
                    }
                }
                if self.sidebar_open && x < self.sidebar_width && y > 0 {
                    if self.sidebar_hit(x, y) {
                        return Ok(());
                    }
                }
                if let Some(sg) = self.splitter_at(x, y) {
                    self.drag = Some(Drag::Splitter { split_id: sg.split_id });
                    return Ok(());
                }
                if let Some(pg) = self.pane_at(x, y) {
                    self.set_focus(pg.pane_id);
                    let inner = pg.inner();
                    let col = x - inner.x + 1;
                    let row = y - inner.y + 1;
                    if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                        if pane.has_mouse_reporting() {
                            let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                            pane.write(&sgr_mouse(b, col, row, false));
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(Drag::Splitter { split_id }) = self.drag {
                    let geom = self.active_geom();
                    if let Some(sg) = geom.splitters.iter().find(|s| s.split_id == split_id) {
                        let ratio = match sg.dir {
                            SplitDir::V => {
                                (x.saturating_sub(sg.area.x)) as f32 / (sg.area.width - 1) as f32
                            }
                            SplitDir::H => {
                                (y.saturating_sub(sg.area.y)) as f32 / (sg.area.height - 1) as f32
                            }
                        };
                        self.sessions[self.active].tree.set_ratio(split_id, ratio);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if let Some(pg) = self.pane_at(x, y) {
                    self.set_focus(pg.pane_id);
                    let inner = pg.inner();
                    let col = x - inner.x + 1;
                    let row = y - inner.y + 1;
                    if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                        if pane.has_mouse_reporting() {
                            let b = if up { 65 } else { 64 };
                            pane.write(&sgr_mouse(b, col, row, false));
                        } else if pane.term.mode().contains(
                            alacritty_terminal::term::TermMode::ALT_SCREEN,
                        ) {
                            pane.write(if up { b"\x1b[A" } else { b"\x1b[B" });
                        } else {
                            pane.scroll(if up { 3 } else { -3 });
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ----- geometry / focus -----

    /// Rect covered by the pane grid (excludes tab row and status bar).
    fn panes_area(&self) -> Rect {
        let (w, h) = self.term_size;
        let x = if self.sidebar_open {
            (self.sidebar_width + 1).min(w.saturating_sub(1))
        } else {
            0
        };
        Rect::new(x, 1, w.saturating_sub(x), h.saturating_sub(2))
    }

    /// Geometry without zoom applied (used for navigation).
    fn tree_geom(&self) -> TreeGeom {
        let mut geom = TreeGeom::default();
        if let Some(root) = &self.sessions[self.active].tree.root {
            layout::compute_geometry(root, self.panes_area(), &mut geom);
        }
        geom
    }

    fn active_geom(&self) -> TreeGeom {
        let mut geom = TreeGeom::default();
        let session = &self.sessions[self.active];
        if let Some(root) = &session.tree.root {
            if session.zoom {
                geom.panes.push(layout::PaneGeom {
                    pane_id: session.tree.focus,
                    rect: self.panes_area(),
                });
            } else {
                layout::compute_geometry(root, self.panes_area(), &mut geom);
            }
        }
        geom
    }

    fn active_pane_dims(&self, pid: u64) -> Option<(u16, u16)> {
        self.active_geom()
            .panes
            .iter()
            .find(|p| p.pane_id == pid)
            .map(|p| {
                let inner = p.inner();
                (inner.width, inner.height)
            })
    }

    fn pane_at(&self, x: u16, y: u16) -> Option<layout::PaneGeom> {
        self.active_geom()
            .panes
            .into_iter()
            .find(|p| p.rect.contains(Position::new(x, y)))
    }

    fn splitter_at(&self, x: u16, y: u16) -> Option<layout::SplitGeom> {
        self.active_geom()
            .splitters
            .into_iter()
            .find(|s| s.rect.contains(Position::new(x, y)))
    }

    fn set_focus(&mut self, pid: u64) {
        if self.sessions[self.active].tree.contains(pid) {
            self.sessions[self.active].tree.focus = pid;
        }
    }

    fn tab_at(&self, x: u16) -> Option<usize> {
        let mut xpos: u16 = 0;
        for (i, s) in self.sessions.iter().enumerate() {
            let w = s.name.chars().count() as u16 + 2;
            if x >= xpos && x < xpos + w {
                return Some(i);
            }
            xpos += w;
        }
        None
    }

    fn pane_dims(&self) -> (u16, u16) {
        let r = self.panes_area();
        (r.width.max(1), r.height.max(1))
    }

    /// Static rows of the sidebar (shared by render + mouse hit-testing).
    fn sidebar_rows(&self) -> Vec<(u16, SidebarRow)> {
        let mut out = Vec::new();
        let mut y: u16 = 1;
        out.push((y, SidebarRow::Header("neomux".into())));
        y += 1;
        out.push((y, SidebarRow::Spacer));
        y += 1;
        out.push((y, SidebarRow::Section("spaces".into())));
        y += 1;
        for (i, _s) in self.sessions.iter().enumerate() {
            out.push((y, SidebarRow::Session(i)));
            y += 1;
        }
        out.push((y, SidebarRow::Spacer));
        y += 1;
        out.push((y, SidebarRow::Section("panes".into())));
        y += 1;
        for (i, s) in self.sessions.iter().enumerate() {
            for pid in s.tree.pane_ids() {
                out.push((y, SidebarRow::Pane(i, pid)));
                y += 1;
            }
        }
        out
    }

    fn sidebar_hit(&mut self, _x: u16, y: u16) -> bool {
        for (ry, row) in self.sidebar_rows() {
            if ry != y {
                continue;
            }
            match row {
                SidebarRow::Session(i) => {
                    self.active = i;
                    return true;
                }
                SidebarRow::Pane(session_idx, pid) => {
                    self.active = session_idx;
                    self.sessions[session_idx].tree.focus = pid;
                    return true;
                }
                _ => return false,
            }
        }
        false
    }

    // ----- rendering -----

    fn frame(&mut self, terminal: &mut Term) -> Result<()> {
        self.poll_exits();
        if self.quit {
            return Ok(());
        }
        let size = terminal.size()?;
        self.term_size = (size.width, size.height);
        let area = Rect::new(0, 0, size.width, size.height);
        let geom = self.active_geom();

        for pg in &geom.panes {
            if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                let inner = pg.inner();
                let key = (inner.width, inner.height);
                if self.last_sizes.get(&pg.pane_id) != Some(&key) {
                    pane.resize(inner.width, inner.height);
                    self.last_sizes.insert(pg.pane_id, key);
                }
            }
        }

        let focused = self.sessions[self.active].tree.focus;
        let geom_ref = &geom;
        terminal.draw(|f| self.render(f, area, geom_ref, focused))?;
        self.place_cursor(terminal, &geom, focused)?;
        Ok(())
    }

    fn render(&self, f: &mut Frame, size: Rect, geom: &TreeGeom, focused: u64) {
        self.render_tabs(f, size);

        let panes_area = self.panes_area();
        fill(f, panes_area, PANEL_BG);

        for pg in &geom.panes {
            let title = self.pane_title(pg.pane_id);
            self.render_pane_frame(f, pg.rect, pg.pane_id == focused, &title);
        }
        for pg in &geom.panes {
            if let Some(pane) = self.panes.get(&pg.pane_id) {
                let inner = pg.inner();
                if inner.width > 0 && inner.height > 0 {
                    pane.render(inner, pg.pane_id == focused, f.buffer_mut());
                    self.render_scrollbar(f, pane, inner);
                }
            }
        }

        if self.sidebar_open {
            self.render_sidebar(f, size);
        }

        self.render_status(f, size);

        if self.mode == Mode::Leader {
            self.render_leader(f, size);
        }
    }

    fn pane_title(&self, pid: u64) -> String {
        match self.panes.get(&pid) {
            Some(p) if p.is_ai => " claude ".to_string(),
            Some(_) => format!(" shell {}", pid),
            None => format!(" pane {}", pid),
        }
    }

    fn render_pane_frame(&self, f: &mut Frame, rect: Rect, focused: bool, title: &str) {
        if rect.width < 3 || rect.height < 3 {
            return;
        }
        let accent = if focused { crate::pane::ACCENT } else { BORDER_IDLE };
        let style = Style::default().fg(accent).bg(PANEL_BG);
        let bold = Style::default()
            .fg(accent)
            .bg(PANEL_BG)
            .add_modifier(ratatui::style::Modifier::BOLD);
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right() - 1, rect.bottom() - 1);
        put(f, x0, y0, "┌", style);
        put(f, x1, y0, "┐", style);
        put(f, x0, y1, "└", style);
        put(f, x1, y1, "┘", style);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", style);
            put(f, x, y1, "─", style);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, "│", style);
            put(f, x1, y, "│", style);
        }
        let mut title = title.to_string();
        let max = rect.width.saturating_sub(2) as usize;
        title.truncate(max);
        for (i, ch) in title.chars().enumerate() {
            put(f, x0 + 1 + i as u16, y0, &ch.to_string(), bold);
        }
    }

    fn render_scrollbar(&self, f: &mut Frame, pane: &Pane, inner: Rect) {
        let grid = pane.term.grid();
        let hist = grid.history_size();
        if hist == 0 {
            return;
        }
        let screen = grid.screen_lines().max(1);
        let total = hist + screen;
        let bar_h = inner.height as usize;
        let thumb = ((screen * bar_h) / total).max(1).min(bar_h);
        let off = grid.display_offset();
        let y_max = bar_h.saturating_sub(thumb);
        let y_start = (off.saturating_mul(y_max)) / hist.max(1);
        let x = inner.x + inner.width.saturating_sub(1);
        for i in 0..bar_h {
            let y = inner.y + i as u16;
            if i >= y_start && i < y_start + thumb {
                put(f, x, y, "▐", Style::default().fg(crate::pane::ACCENT));
            } else {
                put(f, x, y, "░", Style::default().fg(PANEL_SEP));
            }
        }
    }

    fn render_sidebar(&self, f: &mut Frame, size: Rect) {
        let w = self.sidebar_width.min(size.width);
        let area = Rect::new(0, 1, w, size.height.saturating_sub(2));
        fill(f, area, PANEL_BG);
        // Separator between sidebar and panes.
        for y in area.y..(area.y + area.height) {
            put(f, area.x + area.width, y, "│", Style::default().fg(PANEL_SEP));
        }
        for (y, row) in self.sidebar_rows() {
            if y > area.y + area.height {
                break;
            }
            let x = area.x;
            match row {
                SidebarRow::Header(t) => {
                    let style = Style::default()
                        .fg(crate::pane::ACCENT)
                        .bg(PANEL_BG)
                        .add_modifier(ratatui::style::Modifier::BOLD);
                    text(f, x, y, &t, style, w.saturating_sub(1));
                }
                SidebarRow::Spacer => {
                    put(f, x, y, " ", Style::default().bg(PANEL_BG));
                }
                SidebarRow::Section(t) => {
                    let style = Style::default().fg(PANEL_MUTED).bg(PANEL_BG);
                    text(f, x, y, &format!(" {}", t.to_uppercase()), style, w.saturating_sub(1));
                }
                SidebarRow::Session(i) => {
                    let active = i == self.active;
                    let name = &self.sessions[i].name;
                    let (dot, style) = if active {
                        ("●", Style::default().fg(crate::pane::ACCENT).bg(PANEL_BG))
                    } else {
                        ("○", Style::default().fg(PANEL_MUTED).bg(PANEL_BG))
                    };
                    let line = format!(" {} {}", dot, name);
                    text(f, x, y, &line, style, w.saturating_sub(1));
                }
                SidebarRow::Pane(session_idx, pid) => {
                    let active_session = session_idx == self.active
                        && self.sessions[session_idx].tree.focus == pid;
                    let label = match self.panes.get(&pid) {
                        Some(p) if p.is_ai => "claude".to_string(),
                        Some(_) => "shell".to_string(),
                        None => "pane".to_string(),
                    };
                    let prefix = if self.sessions.len() > 1 {
                        format!("{} · ", self.sessions[session_idx].name)
                    } else {
                        String::new()
                    };
                    let (dot, fg) = if active_session {
                        ("●", crate::pane::ACCENT)
                    } else {
                        ("○", PANEL_MUTED)
                    };
                    let line = format!(" {} {}{}", dot, prefix, label);
                    text(f, x, y, &line, Style::default().fg(fg).bg(PANEL_BG), w.saturating_sub(1));
                }
            }
        }
    }

    fn render_tabs(&self, f: &mut Frame, size: Rect) {
        let area = Rect::new(0, 0, size.width, 1);
        fill(f, area, PANEL_BG);
        let mut spans: Vec<Span> = Vec::new();
        for (i, s) in self.sessions.iter().enumerate() {
            let active = i == self.active;
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                s.name.clone(),
                if active {
                    Style::default().fg(RColor::Black).bg(crate::pane::ACCENT)
                } else {
                    Style::default().fg(crate::pane::FG).bg(PANEL_BG)
                },
            ));
            spans.push(Span::raw(" "));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_status(&self, f: &mut Frame, size: Rect) {
        let area = Rect::new(0, size.height.saturating_sub(1), size.width, 1);
        fill(f, area, PANEL_BG);
        let session = &self.sessions[self.active];
        let n = session.tree.pane_count();
        let left = format!(
            " {} · {} · {} panes{}",
            session.name,
            if session.zoom { "zoom" } else { "normal" },
            n,
            if self.sidebar_open { "" } else { " · sidebar hidden" }
        );
        let right = " ctrl+b prefix · v v-split · - h-split · c new · x close · z zoom · h/j/k/l focus · q quit ";
        let width = area.width as usize;
        let right = right.to_string();
        let pad = width.saturating_sub(left.chars().count() + right.chars().count() + 2);
        let line = format!("{}{}{}", left, " ".repeat(pad.min(1024)), right);
        let style = Style::default().fg(crate::pane::FG).bg(PANEL_BG);
        f.render_widget(Paragraph::new(Line::from(Span::styled(line, style))), area);
    }

    fn render_leader(&self, f: &mut Frame, size: Rect) {
        let text = Line::from(Span::styled(
            "PREFIX  v=v-split · -=h-split · c=new · x=close · z=zoom · h/j/k/l=focus · n/p=tab · tab=pane · b=sidebar · q=quit · esc=exit",
            Style::default().fg(RColor::Black).bg(crate::pane::ACCENT),
        ));
        let w = 100;
        let x = size.width.saturating_sub(w) / 2;
        let y = size.height.saturating_sub(3);
        let area = Rect::new(x, y, w, 1);
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(text).alignment(Alignment::Center), area);
    }

    fn place_cursor(&mut self, terminal: &mut Term, geom: &TreeGeom, focused: u64) -> Result<()> {
        if let Some(pg) = geom.panes.iter().find(|p| p.pane_id == focused) {
            if let Some(pane) = self.panes.get(&pg.pane_id) {
                let inner = pg.inner();
                if let Some((cx, cy)) = pane.cursor_pos() {
                    let x = inner.x + cx;
                    let y = inner.y + cy;
                    if x < inner.x + inner.width && y < inner.y + inner.height {
                        terminal.set_cursor_position((x, y))?;
                        terminal.show_cursor()?;
                        return Ok(());
                    }
                }
            }
        }
        terminal.hide_cursor()?;
        Ok(())
    }
}

fn put(f: &mut Frame, x: u16, y: u16, ch: &str, style: Style) {
    let a = f.area();
    if x >= a.width || y >= a.height {
        return;
    }
    let c = f.buffer_mut().cell_mut((x, y)).unwrap();
    c.set_symbol(ch).set_style(style);
}

fn text(f: &mut Frame, x: u16, y: u16, s: &str, style: Style, max_width: u16) {
    for (i, ch) in s.chars().take(max_width as usize).enumerate() {
        put(f, x + i as u16, y, &ch.to_string(), style);
    }
}

fn fill(f: &mut Frame, area: Rect, color: RColor) {
    for y in area.y..(area.y + area.height) {
        for x in area.x..(area.x + area.width) {
            let a = f.area();
            if x >= a.width || y >= a.height {
                continue;
            }
            let c = f.buffer_mut().cell_mut((x, y)).unwrap();
            c.set_symbol(" ").set_style(Style::default().bg(color));
        }
    }
}
