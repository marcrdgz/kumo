#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

// ---------------------------------------------------------------------------
// Directory resolution (XDG-style, Ghostty-like)
//
//   config   $KUMO_CONFIG_DIR | $XDG_CONFIG_HOME/kumo | ~/.config/kumo
//   state    $KUMO_STATE_DIR   | $XDG_STATE_HOME/kumo | ~/.local/state/kumo
//   runtime  $XDG_RUNTIME_DIR/kumo | $TMPDIR/kumo | /tmp/kumo
//
// The config directory is the single source for all user configuration. The
// state directory holds runtime state (workspace.json today; session/socket
// data when the detach server lands). The runtime directory is reserved for
// the future IPC socket.
// ---------------------------------------------------------------------------

/// The directory holding the user's configuration: `~/.config/kumo` by
/// default, overridable via `KUMO_CONFIG_DIR` or `XDG_CONFIG_HOME`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = env_nonempty("KUMO_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(xdg) = env_nonempty("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("kumo");
    }
    home_dir()
        .map(|home| home.join(".config").join("kumo"))
        .unwrap_or_default()
}

/// The directory holding runtime state: `~/.local/state/kumo` by default,
/// overridable via `KUMO_STATE_DIR` or `XDG_STATE_HOME`.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = env_nonempty("KUMO_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(xdg) = env_nonempty("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("kumo");
    }
    home_dir()
        .map(|home| home.join(".local").join("state").join("kumo"))
        .unwrap_or_default()
}

/// The directory reserved for the future detach server's IPC socket:
/// `$XDG_RUNTIME_DIR/kumo` when available, else `$TMPDIR/kumo` or `/tmp/kumo`.
pub fn runtime_dir() -> PathBuf {
    if let Some(runtime) = env_nonempty("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("kumo");
    }
    if let Some(tmp) = env_nonempty("TMPDIR") {
        return PathBuf::from(tmp).join("kumo");
    }
    PathBuf::from("/tmp").join("kumo")
}

/// The main configuration file: `config_dir()/config`.
pub fn config_file() -> PathBuf {
    config_dir().join("config")
}

/// The canonical TOML configuration file (0.4.0+, the schema 1.0 freezes):
/// `config_dir()/config.toml`. Wins over the flat `config` file when both
/// exist.
pub fn config_file_toml() -> PathBuf {
    config_dir().join("config.toml")
}

/// The persisted session state (0.3.0 detach/re-attach): `state_dir()/state.json`.
/// Written atomically by `state::save`; a missing or unreadable file simply
/// means a fresh start.
pub fn state_file() -> PathBuf {
    state_dir().join("state.json")
}

/// The future daemon's IPC socket (0.4.0 client-server): `runtime_dir()/kumo.sock`.
/// Reserved now so 0.3.0's state contract and 0.4.0's daemon agree on the path
/// from day one.
pub fn ipc_socket_path() -> PathBuf {
    runtime_dir().join("kumo.sock")
}

/// The transient resume file the daemon writes before `kumo update` restarts
/// it (`runtime_dir()/resume.json`): the sessions/layout plus each pane's
/// inherited PTY master descriptor, so the restarted daemon adopts the live
/// terminals instead of losing them.
pub fn resume_file() -> PathBuf {
    runtime_dir().join("resume.json")
}

/// Legacy config file (`~/.kumo`) read as a fallback for back-compat.
fn legacy_config_file() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".kumo"))
}

/// Legacy persisted workspace (`~/.kumo/workspace.json`), read as a fallback
/// for back-compat.
fn legacy_workspace_file() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".kumo").join("workspace.json"))
}

// ---------------------------------------------------------------------------
// Configuration model
// ---------------------------------------------------------------------------

/// Sidebar section identifier (sessions / agents).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SidebarSection {
    Sessions,
    Agents,
}

/// Pane/sidebar border style.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BorderStyle {
    Single,
    #[default]
    Rounded,
    Double,
    Heavy,
    Hidden,
}

impl BorderStyle {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "single" => Some(Self::Single),
            "rounded" => Some(Self::Rounded),
            "double" => Some(Self::Double),
            "heavy" => Some(Self::Heavy),
            "hidden" | "none" | "off" => Some(Self::Hidden),
            _ => None,
        }
    }
}

/// Sidebar layout: a two-tab toggle, two stacked panels, or the project-structured view.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SidebarLayout {
    /// Two stacked panels: spaces on top, agents below.
    Divided,
    /// Toggle tabs (`sessions` / `agents`), one section at a time.
    Tabs,
    /// Projects → worktrees with inline agents (finder on `leader+f`) (default).
    #[default]
    Project,
}

impl SidebarLayout {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "divided" | "stacked" => Some(Self::Divided),
            "tabs" | "toggle" => Some(Self::Tabs),
            "project" | "projects" | "explorer" | "navigator" => Some(Self::Project),
            _ => None,
        }
    }
}

/// Which sidebar sections are visible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarSections {
    pub sessions: bool,
    pub agents: bool,
}

impl Default for SidebarSections {
    fn default() -> Self {
        Self { sessions: true, agents: true }
    }
}

/// Border styling for panes and the sidebar separator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarBorders {
    pub style: BorderStyle,
}

impl Default for SidebarBorders {
    fn default() -> Self {
        Self { style: BorderStyle::Rounded }
    }
}

/// Sidebar configuration (`[sidebar]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarConfig {
    /// Ordered list of visible sections (subset/permutation of [Sessions, Agents]).
    pub order: Vec<SidebarSection>,
    pub sections: SidebarSections,
    pub borders: SidebarBorders,
    /// How sidebar sections are arranged (stacked panels or toggle tabs).
    pub layout: SidebarLayout,
    /// Width of the sidebar in columns when `layout = "project"` (draggable; clamped 20..50).
    /// `None` = default (28).
    pub width: Option<u16>,
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            order: vec![SidebarSection::Sessions, SidebarSection::Agents],
            sections: SidebarSections::default(),
            borders: SidebarBorders::default(),
            layout: SidebarLayout::Project,
            width: None,
        }
    }
}

/// Status bar widget identifier.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum StatusWidget {
    Mode,
    Menu,
    Session,
    Branch,
    AgentStatus,
    Hostname,
    Clock,
}

impl StatusWidget {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mode" => Some(Self::Mode),
            "menu" => Some(Self::Menu),
            "session" => Some(Self::Session),
            "branch" => Some(Self::Branch),
            "agent" | "agents" | "agent_status" | "agent-status" => Some(Self::AgentStatus),
            "hostname" | "host" => Some(Self::Hostname),
            "clock" | "time" => Some(Self::Clock),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mode => "mode",
            Self::Menu => "menu",
            Self::Session => "session",
            Self::Branch => "branch",
            Self::AgentStatus => "agent_status",
            Self::Hostname => "hostname",
            Self::Clock => "clock",
        }
    }
}

/// Clock widget options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockWidgetConfig {
    /// `strftime`-like format, default `"%H:%M"` (minute granularity).
    pub format: String,
}

impl Default for ClockWidgetConfig {
    fn default() -> Self {
        Self { format: "%H:%M".to_string() }
    }
}

/// Branch widget options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchWidgetConfig {
    pub show_ahead_behind: bool,
    pub max_len: usize,
}

impl Default for BranchWidgetConfig {
    fn default() -> Self {
        Self { show_ahead_behind: true, max_len: 20 }
    }
}

/// Agent widget display style.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AgentWidgetStyle {
    /// `"◉1 blocked · ●2 working"` (default).
    #[default]
    Counts,
    Dots,
    List,
}

impl AgentWidgetStyle {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "counts" | "count" => Some(Self::Counts),
            "dots" | "dot" => Some(Self::Dots),
            "list" => Some(Self::List),
            _ => None,
        }
    }
}

/// Agent widget options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentWidgetConfig {
    pub style: AgentWidgetStyle,
    pub only_blocked: bool,
}

impl Default for AgentWidgetConfig {
    fn default() -> Self {
        Self { style: AgentWidgetStyle::Counts, only_blocked: false }
    }
}

/// Hostname widget options.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HostnameStyle {
    #[default]
    Short,
    Fqdn,
}

impl HostnameStyle {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "short" => Some(Self::Short),
            "fqdn" | "full" => Some(Self::Fqdn),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostnameWidgetConfig {
    pub style: HostnameStyle,
    pub show_user: bool,
    pub only_ssh: bool,
}

impl Default for HostnameWidgetConfig {
    fn default() -> Self {
        Self { style: HostnameStyle::Short, show_user: false, only_ssh: false }
    }
}

/// Session widget options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionWidgetConfig {
    pub show_tabs: bool,
    pub show_panes: bool,
    pub show_zoom: bool,
}

impl Default for SessionWidgetConfig {
    fn default() -> Self {
        Self { show_tabs: true, show_panes: true, show_zoom: true }
    }
}

/// Per-widget option bag for the status bar.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StatusBarWidgets {
    pub clock: ClockWidgetConfig,
    pub branch: BranchWidgetConfig,
    pub agent: AgentWidgetConfig,
    pub hostname: HostnameWidgetConfig,
    pub session: SessionWidgetConfig,
}

/// Where an attached viewer draws its agent-lifecycle toast stack
/// (`[notifications] position`). Defaults to `TopRight`; `Off` is the kill
/// switch — no toasts are raised at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ToastPosition {
    /// Top-right corner, below the tab bar (default).
    #[default]
    TopRight,
    /// Top-left corner, below the tab bar.
    TopLeft,
    /// Bottom-right corner, above the status bar.
    BottomRight,
    /// Bottom-left corner, above the status bar.
    BottomLeft,
    /// Horizontally + vertically centered as a group.
    Center,
    /// Never raise toasts (`never`/`off`). The audible chime is unaffected.
    Off,
}

impl ToastPosition {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "top-right" | "top_right" | "topright" | "tr" => Some(Self::TopRight),
            "top-left" | "top_left" | "topleft" | "tl" => Some(Self::TopLeft),
            "bottom-right" | "bottom_right" | "bottomright" | "br" => Some(Self::BottomRight),
            "bottom-left" | "bottom_left" | "bottomleft" | "bl" => Some(Self::BottomLeft),
            "center" | "centre" | "middle" => Some(Self::Center),
            "never" | "off" | "none" | "disabled" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Agent-notification policy for lifecycle transitions (`[notifications]`).
/// The channel is a transient **toast** in every attached kumo viewer (no
/// OS-level popups; the audible chime still covers detached sessions).
/// Toasts are on by default at the top-right corner; `[notifications]
/// position = "off"` silences them (`KUMO_NO_NOTIFY=1` does the same). The
/// chime is gated separately by `sound` / `KUMO_NO_SOUND=1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationsConfig {
    /// Notify when a working agent becomes blocked (default: true).
    pub blocked: bool,
    /// Notify when a working agent finishes / goes idle (default: true).
    pub finished: bool,
    /// Where the viewer anchors the toast stack; `Off` disables toasts
    /// entirely (default: top-right).
    pub position: ToastPosition,
    /// Whether transitions play the audible chime (default: true).
    pub sound: bool,
}

impl NotificationsConfig {
    /// Whether toasts should be raised at all (`position != Off`).
    pub fn toasts_enabled(&self) -> bool {
        self.position != ToastPosition::Off
    }
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            blocked: true,
            finished: true,
            position: ToastPosition::default(),
            sound: true,
        }
    }
}

/// Status bar configuration (`[status_bar]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusBarConfig {
    pub enabled: bool,
    pub left: Vec<StatusWidget>,
    pub center: Vec<StatusWidget>,
    pub right: Vec<StatusWidget>,
    pub widgets: StatusBarWidgets,
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            left: vec![StatusWidget::Mode, StatusWidget::Menu, StatusWidget::Session],
            center: vec![StatusWidget::Branch],
            right: vec![StatusWidget::AgentStatus, StatusWidget::Clock],
            widgets: StatusBarWidgets::default(),
        }
    }
}

/// Worktree configuration (`[worktree]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeConfig {
    /// Gitignored directories to symlink/clone-copy into each new worktree.
    pub shared_dirs: Vec<PathBuf>,
    /// Expose `KUMO_SOCKET_PATH`/`KUMO_BIN_PATH` to spawned panes.
    pub expose_socket: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self { shared_dirs: Vec::new(), expose_socket: true }
    }
}

/// Checkpoint skill configuration (`[checkpoints]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointsConfig {
    /// Whether the checkpoint skill is auto-installed (default: true).
    pub enabled: bool,
}

impl Default for CheckpointsConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Parsed user configuration. Mirrors the flat `key = value` config file;
/// future knobs (theme, leader, keymaps, status bar) will extend this struct.
#[derive(Clone)]
pub struct Config {
    /// Program + args for the AI pane.
    pub ai_cmd: Option<(String, Vec<String>)>,
    /// Login shell used to spawn panes.
    pub shell: Option<String>,
    /// Whether the startup update check runs (default: true).
    pub update_check: bool,
    /// The leader chord, e.g. `ctrl+b` or `ctrl+space`. `None` means the
    /// built-in default (Ctrl+B).
    pub leader: Option<String>,
    /// `[keymap.bindings]` overrides: key/chord string → action id
    /// (e.g. `"s"` → `"split-vertical"`).
    pub keymap_bindings: HashMap<String, String>,
    /// Policy for the session's working directory (`[terminal] new-cwd`),
    /// defaulting to `Follow`.
    pub new_cwd: NewCwdMode,
    /// Fixed working directory used when `new-cwd = "fixed"`
    /// (`[terminal] fixed-cwd`).
    pub fixed_cwd: Option<PathBuf>,
    /// Selected theme name (`[theme] name = "dracula"` or `theme = "dracula"` flat).
    pub theme: Option<String>,
    /// Custom theme defined in `[theme.custom]` (full palette + chrome overrides).
    pub custom_theme: Option<crate::theme::OwnedTheme>,
    /// Sidebar toggle/order + border styling (`[sidebar]`).
    pub sidebar: SidebarConfig,
    /// Status bar widget layout (`[status_bar]`).
    pub status_bar: StatusBarConfig,
    /// Agent notifications for lifecycle transitions: transient corner
    /// toasts in attached viewers (`[notifications]`).
    pub notifications: NotificationsConfig,
    /// Worktree isolation (`[worktree]`).
    pub worktree: WorktreeConfig,
    /// Checkpoint skill (`[checkpoints]`).
    pub checkpoints: CheckpointsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ai_cmd: None,
            shell: None,
            update_check: true,
            leader: None,
            keymap_bindings: HashMap::new(),
            new_cwd: NewCwdMode::default(),
            fixed_cwd: None,
            theme: None,
            custom_theme: None,
            sidebar: SidebarConfig::default(),
            status_bar: StatusBarConfig::default(),
            notifications: NotificationsConfig::default(),
            worktree: WorktreeConfig::default(),
            checkpoints: CheckpointsConfig::default(),
        }
    }
}

/// How the session's working directory is chosen (`[terminal] new-cwd`). The
/// resolved [`NewCwd`] (where the fixed path lives) is produced by
/// [`new_cwd()`].
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum NewCwdMode {
    /// Follow the focused pane's detected cwd live (default).
    #[default]
    Follow,
    /// The directory kumo was launched from, frozen at session creation.
    Current,
    /// `$HOME`.
    Home,
    /// An explicit `[terminal] fixed-cwd` path.
    Fixed,
}

/// Resolved `new-cwd` policy, carrying the fixed path when configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewCwd {
    Follow,
    Current,
    Home,
    Fixed(PathBuf),
}

/// Parse a `new-cwd` value. Unknown values warn and fall back to `Follow`.
fn parse_new_cwd(v: &str) -> NewCwdMode {
    match v.trim().to_ascii_lowercase().as_str() {
        "follow" => NewCwdMode::Follow,
        "current" => NewCwdMode::Current,
        "home" => NewCwdMode::Home,
        "fixed" => NewCwdMode::Fixed,
        other => {
            log::warn!("kumo: ignoring invalid new-cwd {other:?}; using follow");
            NewCwdMode::Follow
        }
    }
}

fn build_custom_theme(raw: CustomThemeRaw) -> Option<crate::theme::OwnedTheme> {
    // At least one field must be set to consider a custom theme present.
    let has_any = raw.name.is_some()
        || raw.palette.is_some()
        || raw.term_fg.is_some()
        || raw.term_bg.is_some()
        || raw.term_cursor.is_some()
        || raw.fg.is_some()
        || raw.accent.is_some()
        || raw.secondary.is_some()
        || raw.panel_sep.is_some()
        || raw.panel_muted.is_some()
        || raw.border_idle.is_some()
        || raw.green.is_some()
        || raw.orange.is_some()
        || raw.red.is_some()
        || raw.input_bg.is_some();
    if !has_any {
        return None;
    }
    // Start from the default theme so missing values have sensible fallbacks.
    let mut base = crate::theme::OwnedTheme::from(crate::theme::THEMES[crate::theme::DEFAULT_THEME_IDX]);
    if let Some(name) = raw.name {
        let n = name.trim();
        if !n.is_empty() {
            base.name = n.to_string();
        } else {
            base.name = "Custom".to_string();
        }
    } else {
        base.name = "Custom".to_string();
    }
    if let Some(pal) = raw.palette {
        for (i, s) in pal.into_iter().enumerate().take(16) {
            match crate::color::parse_hex(&s) {
                Some(c) => base.palette[i] = c,
                None => log::warn!("kumo: ignoring invalid palette[{i}] {s:?}"),
            }
        }
        if base.palette.len() != 16 {
            // Should never happen; array always 16.
        }
    }
    let set_rgb = |field: &mut crate::color::ColorRgb, raw_val: Option<String>, label: &str| {
        if let Some(s) = raw_val {
            if let Some(c) = crate::color::parse_hex(&s) {
                *field = c;
            } else {
                log::warn!("kumo: ignoring invalid {label} {s:?}");
            }
        }
    };
    let set_rcolor = |field: &mut ratatui::style::Color, raw_val: Option<String>, label: &str| {
        if let Some(s) = raw_val {
            if let Some(c) = crate::theme::parse_rcolor(&s) {
                *field = c;
            } else {
                log::warn!("kumo: ignoring invalid {label} {s:?}");
            }
        }
    };
    set_rgb(&mut base.term_fg, raw.term_fg, "term_fg");
    set_rgb(&mut base.term_bg, raw.term_bg, "term_bg");
    set_rgb(&mut base.term_cursor, raw.term_cursor, "term_cursor");
    set_rcolor(&mut base.fg, raw.fg, "fg");
    set_rcolor(&mut base.accent, raw.accent, "accent");
    set_rcolor(&mut base.secondary, raw.secondary, "secondary");
    set_rcolor(&mut base.panel_sep, raw.panel_sep, "panel_sep");
    set_rcolor(&mut base.panel_muted, raw.panel_muted, "panel_muted");
    set_rcolor(&mut base.border_idle, raw.border_idle, "border_idle");
    set_rcolor(&mut base.green, raw.green, "green");
    set_rcolor(&mut base.orange, raw.orange, "orange");
    set_rcolor(&mut base.red, raw.red, "red");
    set_rcolor(&mut base.input_bg, raw.input_bg, "input_bg");
    Some(base)
}

impl Config {
    fn apply_sidebar(&mut self, raw: SidebarSectionRaw) {
        // order: Vec<String> -> Vec<SidebarSection>
        if let Some(order) = raw.order {
            let mut parsed = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for s in order {
                let sec = match s.trim().to_ascii_lowercase().as_str() {
                    "sessions" | "session" => SidebarSection::Sessions,
                    "agents" | "agent" => SidebarSection::Agents,
                    other => {
                        log::warn!("kumo: ignoring unknown sidebar order entry {other:?}");
                        continue;
                    }
                };
                if seen.insert(sec) {
                    parsed.push(sec);
                }
            }
            // Fill missing defaults so both sections remain reachable if user
            // only listed one; preserves toggle ability.
            for sec in [SidebarSection::Sessions, SidebarSection::Agents] {
                if !parsed.contains(&sec) {
                    parsed.push(sec);
                }
            }
            if !parsed.is_empty() {
                self.sidebar.order = parsed;
            }
        }
        if let Some(sections) = raw.sections {
            if let Some(v) = sections.sessions {
                self.sidebar.sections.sessions = v;
            }
            if let Some(v) = sections.agents {
                self.sidebar.sections.agents = v;
            }
            // warn if both hidden
            if !self.sidebar.sections.sessions && !self.sidebar.sections.agents {
                log::warn!("kumo: [sidebar.sections] both hidden; at least one will be shown");
                self.sidebar.sections.sessions = true;
            }
        }
        if let Some(borders) = raw.borders {
            if let Some(s) = borders.style {
                if let Some(st) = BorderStyle::parse(&s) {
                    self.sidebar.borders.style = st;
                } else {
                    log::warn!("kumo: ignoring invalid sidebar.borders.style {s:?}");
                }
            }
        }
        if let Some(layout) = raw.layout {
            if let Some(l) = SidebarLayout::parse(&layout) {
                self.sidebar.layout = l;
            } else {
                log::warn!("kumo: ignoring invalid sidebar.layout {layout:?}");
            }
        }
        if let Some(w) = raw.width {
            if (20..=50).contains(&w) {
                self.sidebar.width = Some(w);
            } else {
                log::warn!("kumo: ignoring out-of-range sidebar.width {w}; expected 20..50");
            }
        }
        // Legacy flat keys for borders style
        // handled in from_map; toml is canonical.
    }

    fn apply_status_bar(&mut self, raw: StatusBarRaw) {
        if let Some(v) = raw.enabled {
            self.status_bar.enabled = v;
        }
        // left / center / right: Vec<String> -> Vec<StatusWidget>
        let parse_widgets = |vals: Vec<String>| -> Vec<StatusWidget> {
            let mut out = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for s in vals {
                if let Some(w) = StatusWidget::parse(&s) {
                    if seen.insert(w) {
                        out.push(w);
                    }
                } else {
                    log::warn!("kumo: ignoring unknown status_bar widget {s:?}");
                }
            }
            out
        };
        if let Some(vals) = raw.left {
            self.status_bar.left = parse_widgets(vals);
        }
        if let Some(vals) = raw.center {
            self.status_bar.center = parse_widgets(vals);
        }
        if let Some(vals) = raw.right {
            self.status_bar.right = parse_widgets(vals);
        }
        if let Some(widgets) = raw.widgets {
            if let Some(clock) = widgets.clock {
                if let Some(fmt) = clock.format {
                    if !fmt.trim().is_empty() {
                        self.status_bar.widgets.clock.format = fmt;
                    }
                }
            }
            if let Some(branch) = widgets.branch {
                if let Some(v) = branch.show_ahead_behind {
                    self.status_bar.widgets.branch.show_ahead_behind = v;
                }
                if let Some(v) = branch.max_len {
                    if v > 0 {
                        self.status_bar.widgets.branch.max_len = v as usize;
                    }
                }
            }
            if let Some(agent) = widgets.agent {
                if let Some(s) = agent.style {
                    if let Some(st) = AgentWidgetStyle::parse(&s) {
                        self.status_bar.widgets.agent.style = st;
                    } else {
                        log::warn!("kumo: ignoring invalid status_bar.widgets.agent.style {s:?}");
                    }
                }
                if let Some(v) = agent.only_blocked {
                    self.status_bar.widgets.agent.only_blocked = v;
                }
            }
            if let Some(host) = widgets.hostname {
                if let Some(s) = host.style {
                    if let Some(st) = HostnameStyle::parse(&s) {
                        self.status_bar.widgets.hostname.style = st;
                    } else {
                        log::warn!("kumo: ignoring invalid status_bar.widgets.hostname.style {s:?}");
                    }
                }
                if let Some(v) = host.show_user {
                    self.status_bar.widgets.hostname.show_user = v;
                }
                if let Some(v) = host.only_ssh {
                    self.status_bar.widgets.hostname.only_ssh = v;
                }
            }
            if let Some(sess) = widgets.session {
                if let Some(v) = sess.show_tabs {
                    self.status_bar.widgets.session.show_tabs = v;
                }
                if let Some(v) = sess.show_panes {
                    self.status_bar.widgets.session.show_panes = v;
                }
                if let Some(v) = sess.show_zoom {
                    self.status_bar.widgets.session.show_zoom = v;
                }
            }
        }
        if self.status_bar.enabled
            && self.status_bar.left.is_empty()
            && self.status_bar.center.is_empty()
            && self.status_bar.right.is_empty()
        {
            log::warn!("kumo: [status_bar] all slots empty; using default session widget");
            self.status_bar.left = vec![StatusWidget::Session];
        }
    }

    /// Normalize the new-cwd policy: a `Fixed` mode needs a valid `fixed-cwd`
    /// directory, else it falls back to `Current` with a warning.
    fn normalize_new_cwd(&mut self) {
        if self.new_cwd == NewCwdMode::Fixed {
            let ok = self.fixed_cwd.as_ref().map(|p| p.is_dir()).unwrap_or(false);
            if !ok {
                log::warn!("kumo: new-cwd = \"fixed\" needs a valid fixed-cwd; using current");
                self.new_cwd = NewCwdMode::Current;
            }
        }
    }

    fn from_map(map: &HashMap<String, String>) -> Self {
        let mut cfg = Config::default();
        if let Some(v) = map.get("ai-cmd").or_else(|| map.get("ai_cmd")) {
            let v = unquote(v);
            if !v.is_empty() {
                cfg.ai_cmd = Some(split_cmd(v));
            }
        }
        if let Some(v) = map.get("shell") {
            let v = unquote(v);
            if !v.is_empty() {
                cfg.shell = Some(v.to_string());
            }
        }
        cfg.update_check = map
            .get("update-check")
            .map(|v| !matches!(unquote(v).trim().to_ascii_lowercase().as_str(), "false" | "0" | "no" | "off" | "never"))
            .unwrap_or(true);
        cfg.notifications.sound = map
            .get("agent-sound")
            .map(|v| !matches!(unquote(v).trim().to_ascii_lowercase().as_str(), "false" | "0" | "no" | "off"))
            .unwrap_or(true);
        if let Some(v) = map.get("leader") {
            let v = unquote(v);
            if !v.is_empty() {
                cfg.leader = Some(v.to_string());
            }
        }
        if let Some(v) = map.get("new-cwd") {
            let v = unquote(v);
            if !v.is_empty() {
                cfg.new_cwd = parse_new_cwd(v);
            }
        }
        if let Some(v) = map.get("fixed-cwd") {
            let v = unquote(v);
            if !v.is_empty() {
                cfg.fixed_cwd = Some(PathBuf::from(v));
            }
        }
        if let Some(v) = map.get("theme").or_else(|| map.get("theme.name")) {
            let v = unquote(v);
            if !v.is_empty() {
                cfg.theme = Some(v.to_string());
            }
        }
        cfg.normalize_new_cwd();
        cfg
    }

    /// Overlay typed TOML values on top of the flat-merged config. Only keys
    /// present in the TOML file are touched, so flat and legacy fallbacks keep
    /// supplying everything else.
    fn apply_toml(&mut self, toml: TomlConfig) {
        if let Some(v) = toml.ai_cmd {
            if !v.is_empty() {
                self.ai_cmd = Some(split_cmd(&v));
            }
        }
        // `[terminal] shell` is canonical; a top-level `shell` is accepted as a
        // deprecated alias, but the section wins (same pattern as `leader`).
        let shell = toml
            .terminal
            .as_ref()
            .and_then(|t| t.shell.clone())
            .or(toml.shell);
        if let Some(v) = shell {
            if !v.is_empty() {
                self.shell = Some(v);
            }
        }
        if let Some(v) = toml.update_check {
            self.update_check = v;
        }
        // `[notifications] sound` is canonical; a top-level `agent-sound` is
        // accepted as a deprecated alias, but the section wins.
        if let Some(v) = toml.agent_sound {
            self.notifications.sound = v;
        }
        // The leader lives in the `[keymap]` table; a top-level `leader` is
        // accepted as a deprecated alias, but the table wins.
        let leader = toml
            .keymap
            .as_ref()
            .and_then(|k| k.leader.clone())
            .or(toml.leader_alias);
        if let Some(v) = leader {
            if !v.is_empty() {
                self.leader = Some(v);
            }
        }
        if let Some(v) = toml.keymap.and_then(|k| k.bindings) {
            self.keymap_bindings.extend(v);
        }
        if let Some(terminal) = toml.terminal {
            if let Some(v) = terminal.new_cwd {
                if !v.is_empty() {
                    self.new_cwd = parse_new_cwd(&v);
                }
            }
            if let Some(v) = terminal.fixed_cwd {
                self.fixed_cwd = Some(v);
            }
        }
        // Theme: `theme = "name"` string or `[theme] name = "name"` plus `[theme.custom]`.
        if let Some(tv) = toml.theme {
            match tv {
                ThemeValue::Simple(s) => {
                    if !s.trim().is_empty() {
                        self.theme = Some(s);
                    }
                }
                ThemeValue::Table(tbl) => {
                    let ThemeSection { name, custom } = *tbl;
                    if let Some(n) = name {
                        if !n.trim().is_empty() {
                            self.theme = Some(n);
                        }
                    }
                    if let Some(raw) = custom {
                        if let Some(owned) = build_custom_theme(raw) {
                            self.custom_theme = Some(owned);
                        }
                    }
                }
            }
        }
        // Allow bare `[theme.custom]` without a parent `[theme] name` key when parsed as `themes`
        // alias - toml may represent it as `theme` table with custom only; already handled.
        // Also support top-level `[custom]`? Not needed.
        if let Some(sidebar) = toml.sidebar {
            self.apply_sidebar(sidebar);
        }
        if let Some(status_bar) = toml.status_bar {
            self.apply_status_bar(status_bar);
        }
        if let Some(notif) = toml.notifications {
            if let Some(v) = notif.blocked {
                self.notifications.blocked = v;
            }
            if let Some(v) = notif.finished {
                self.notifications.finished = v;
            }
            if let Some(v) = notif.position {
                if let Some(pos) = ToastPosition::parse(&v) {
                    self.notifications.position = pos;
                } else {
                    log::warn!("kumo: ignoring invalid notifications.position {v:?}");
                }
            }
            if let Some(v) = notif.sound {
                self.notifications.sound = v;
            }
        }
        if let Some(wt) = toml.worktree {
            if let Some(dirs) = wt.shared_dirs.or(wt._shared_dirs_camel) {
                let mut out = Vec::new();
                for raw in dirs {
                    let trimmed = raw.trim().to_string();
                    if trimmed.is_empty() { continue; }
                    let p = PathBuf::from(&trimmed);
                    if p.is_absolute() {
                        log::warn!("kumo: ignoring absolute worktree.shared-dirs entry {trimmed:?}");
                        continue;
                    }
                    if trimmed.contains("..") {
                        log::warn!("kumo: ignoring worktree.shared-dirs entry with '..' {trimmed:?}");
                        continue;
                    }
                    out.push(p);
                }
                self.worktree.shared_dirs = out;
            }
            if let Some(v) = wt.expose_socket {
                self.worktree.expose_socket = v;
            }
        }
        if let Some(cp) = toml.checkpoints {
            if let Some(v) = cp.enabled {
                self.checkpoints.enabled = v;
            }
        }
        self.normalize_new_cwd();
    }
}

/// Raw TOML for `[sidebar]` — all optional, unknown keys ignored.
#[derive(Default, serde::Deserialize, Debug)]
pub struct SidebarSectionRaw {
    pub order: Option<Vec<String>>,
    pub sections: Option<SidebarSectionsRaw>,
    pub borders: Option<SidebarBordersRaw>,
    pub layout: Option<String>,
    pub width: Option<u16>,
}

#[derive(Default, serde::Deserialize, Debug)]
pub struct SidebarSectionsRaw {
    pub sessions: Option<bool>,
    pub agents: Option<bool>,
}

#[derive(Default, serde::Deserialize, Debug)]
pub struct SidebarBordersRaw {
    pub style: Option<String>,
}

/// Raw TOML for `[status_bar]` — all optional, unknown keys ignored.
#[derive(Default, serde::Deserialize, Debug)]
pub struct StatusBarRaw {
    pub enabled: Option<bool>,
    pub left: Option<Vec<String>>,
    pub center: Option<Vec<String>>,
    pub right: Option<Vec<String>>,
    pub widgets: Option<StatusBarWidgetsRaw>,
}

#[derive(Default, serde::Deserialize, Debug)]
pub struct StatusBarWidgetsRaw {
    pub clock: Option<ClockRaw>,
    pub branch: Option<BranchRaw>,
    pub agent: Option<AgentRaw>,
    pub hostname: Option<HostnameRaw>,
    pub session: Option<SessionRaw>,
}

#[derive(Default, serde::Deserialize, Debug)]
pub struct ClockRaw {
    pub format: Option<String>,
}

#[derive(Default, serde::Deserialize, Debug)]
pub struct BranchRaw {
    pub show_ahead_behind: Option<bool>,
    pub max_len: Option<u64>,
}

#[derive(Default, serde::Deserialize, Debug)]
pub struct AgentRaw {
    pub style: Option<String>,
    pub only_blocked: Option<bool>,
}

#[derive(Default, serde::Deserialize, Debug)]
pub struct HostnameRaw {
    pub style: Option<String>,
    pub show_user: Option<bool>,
    pub only_ssh: Option<bool>,
}

#[derive(Default, serde::Deserialize, Debug)]
pub struct SessionRaw {
    pub show_tabs: Option<bool>,
    pub show_panes: Option<bool>,
    pub show_zoom: Option<bool>,
}

/// Raw TOML for `[notifications]` — all optional, unknown keys ignored. The
/// finished channel accepts `idle`/`done` as aliases; the chime accepts
/// `alert-sound`/`alert_sound` spellings.
#[derive(Default, serde::Deserialize, Debug)]
pub struct NotificationsRaw {
    pub blocked: Option<bool>,
    #[serde(alias = "idle", alias = "done")]
    pub finished: Option<bool>,
    /// Toast anchor corner: `top-right` (default), `top-left`,
    /// `bottom-right`, `bottom-left`, `center`, or `off`.
    pub position: Option<String>,
    /// Whether transitions play the audible chime (default: true). Canonical
    /// home of the deprecated top-level `agent-sound` key.
    #[serde(alias = "alert-sound", alias = "alert_sound")]
    pub sound: Option<bool>,
}

/// The `[keymap]` table: keyboard configuration (leader chord today, bindings
/// later). Unknown keys are ignored (serde default).
#[derive(Default, serde::Deserialize)]
struct Keymap {
    /// Leader chord that enters leader mode, e.g. `ctrl+b` or `ctrl+space`.
    leader: Option<String>,
    /// Binding overrides: key/chord string → action id.
    #[serde(rename = "bindings")]
    bindings: Option<HashMap<String, String>>,
}

/// The `[terminal]` table: terminal-behavior knobs (shell, session cwd
/// policy). Unknown keys are ignored (serde default).
#[derive(Default, serde::Deserialize)]
struct TerminalSection {
    /// Login shell used to spawn panes (canonical home; a top-level `shell`
    /// is a deprecated alias).
    shell: Option<String>,
    /// Session working-directory policy: `follow` (default), `home`,
    /// `current`, or `fixed`.
    #[serde(rename = "new-cwd")]
    new_cwd: Option<String>,
    /// Directory used when `new-cwd = "fixed"`.
    #[serde(rename = "fixed-cwd")]
    fixed_cwd: Option<PathBuf>,
}

/// Custom theme values under `[theme.custom]`. Every field is optional and
/// accepts hex strings like `#rrggbb`, `rrggbb` or `#rgb`. Unknown keys ignored.
#[derive(Default, serde::Deserialize, Clone, Debug)]
pub struct CustomThemeRaw {
    pub name: Option<String>,
    pub palette: Option<Vec<String>>,
    #[serde(rename = "term-fg", alias = "term_fg")]
    pub term_fg: Option<String>,
    #[serde(rename = "term-bg", alias = "term_bg")]
    pub term_bg: Option<String>,
    #[serde(rename = "term-cursor", alias = "term_cursor")]
    pub term_cursor: Option<String>,
    pub fg: Option<String>,
    pub accent: Option<String>,
    pub secondary: Option<String>,
    #[serde(rename = "panel-sep", alias = "panel_sep")]
    pub panel_sep: Option<String>,
    #[serde(rename = "panel-muted", alias = "panel_muted")]
    pub panel_muted: Option<String>,
    #[serde(rename = "border-idle", alias = "border_idle")]
    pub border_idle: Option<String>,
    pub green: Option<String>,
    pub orange: Option<String>,
    pub red: Option<String>,
    #[serde(rename = "input-bg", alias = "input_bg")]
    pub input_bg: Option<String>,
}

#[derive(Default, serde::Deserialize)]
struct ThemeSection {
    name: Option<String>,
    custom: Option<CustomThemeRaw>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ThemeValue {
    Simple(String),
    Table(Box<ThemeSection>),
}

/// Raw TOML for `[worktree]` — per-repo shared dirs etc.
#[derive(Default, serde::Deserialize, Debug)]
pub struct WorktreeRaw {
    #[serde(rename = "shared-dirs", alias = "shared_dirs")]
    pub shared_dirs: Option<Vec<String>>,
    #[serde(rename = "sharedDirs", alias = "sharedDirs")]
    _shared_dirs_camel: Option<Vec<String>>,
    #[serde(rename = "expose-socket", alias = "expose_socket")]
    pub expose_socket: Option<bool>,
}

/// Raw TOML for `[checkpoints]` — skill auto-install toggle.
#[derive(Default, serde::Deserialize, Debug)]
pub struct CheckpointsRaw {
    pub enabled: Option<bool>,
}

/// Typed view of the canonical `config.toml`. Unknown keys are ignored (serde
/// default), and `ai_cmd` stays accepted as an alias of `ai-cmd`.
#[derive(Default, serde::Deserialize)]
struct TomlConfig {
    #[serde(rename = "ai-cmd", alias = "ai_cmd")]
    ai_cmd: Option<String>,
    /// Deprecated top-level alias of `[terminal].shell`.
    shell: Option<String>,
    #[serde(rename = "update-check")]
    update_check: Option<bool>,
    #[serde(rename = "agent-sound")]
    agent_sound: Option<bool>,
    #[serde(rename = "keymap")]
    keymap: Option<Keymap>,
    /// Deprecated top-level alias of `[keymap].leader`.
    #[serde(rename = "leader")]
    leader_alias: Option<String>,
    #[serde(rename = "terminal")]
    terminal: Option<TerminalSection>,
    #[serde(rename = "theme")]
    theme: Option<ThemeValue>,
    #[serde(rename = "sidebar")]
    sidebar: Option<SidebarSectionRaw>,
    #[serde(rename = "status_bar", alias = "status-bar")]
    status_bar: Option<StatusBarRaw>,
    #[serde(rename = "notifications")]
    notifications: Option<NotificationsRaw>,
    #[serde(rename = "worktree")]
    worktree: Option<WorktreeRaw>,
    #[serde(rename = "checkpoints")]
    checkpoints: Option<CheckpointsRaw>,
}

/// Load and merge the configuration. Precedence: `config.toml` wins over the
/// flat `config` file, which wins over the legacy `~/.kumo` file; keys absent
/// from a higher-priority source fall back to the next. The flat formats keep
/// reading as fallbacks for back-compat.
fn load_config() -> Config {
    let mut map = HashMap::new();
    if let Some(legacy) = legacy_config_file() {
        if let Ok(parsed) = parse_flat(&legacy) {
            map.extend(parsed);
        }
    }
    if let Ok(parsed) = parse_flat(&config_file()) {
        map.extend(parsed);
    }
    let mut cfg = Config::from_map(&map);
    if let Some(toml) = parse_toml(&config_file_toml()) {
        cfg.apply_toml(toml);
    }
    cfg
}

fn current_config_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = legacy_config_file() {
        v.push(p);
    }
    v.push(config_file());
    v.push(config_file_toml());
    v
}

struct CacheEntry {
    config: Config,
    mtimes: HashMap<PathBuf, Option<SystemTime>>,
    at: Instant,
}

static CONFIG_CACHE: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();

#[cfg(test)]
fn cached_config() -> Config {
    // Tests mutate env/files under TEST_ENV_LOCK and expect immediate
    // visibility; bypass the mtime cache.
    load_config()
}

#[cfg(not(test))]
fn cached_config() -> Config {
    let paths = current_config_paths();
    let mut current_mtimes: HashMap<PathBuf, Option<SystemTime>> = HashMap::new();
    for p in &paths {
        current_mtimes.insert(p.clone(), std::fs::metadata(p).and_then(|m| m.modified()).ok());
    }

    let cache = CONFIG_CACHE.get_or_init(|| Mutex::new(None));
    // Fast path: check under lock
    {
        let guard = cache.lock().unwrap();
        if let Some(entry) = guard.as_ref() {
            if entry.mtimes == current_mtimes {
                return entry.config.clone();
            }
        }
    }
    // Miss: load fresh (without holding lock)
    let config = load_config();
    let mut guard = cache.lock().unwrap();
    // Double-check after load (another thread may have filled)
    if let Some(entry) = guard.as_ref() {
        if entry.mtimes == current_mtimes {
            return entry.config.clone();
        }
    }
    *guard = Some(CacheEntry {
        config: config.clone(),
        mtimes: current_mtimes,
        at: Instant::now(),
    });
    config
}

/// Invalidate the config cache (used by `kumo reload` and tests).
pub fn invalidate_cache() {
    if let Some(cache) = CONFIG_CACHE.get() {
        let _ = cache.lock().map(|mut g| *g = None);
    }
}

// ---------------------------------------------------------------------------
// TOML parser (canonical format)
// ---------------------------------------------------------------------------

/// Parse `config.toml`. Missing files return `None` (no TOML present); invalid
/// TOML logs a warning and falls back to the flat files so a broken file never
/// bricks the terminal.
fn parse_toml(path: &Path) -> Option<TomlConfig> {
    let content = std::fs::read_to_string(path).ok()?;
    match toml::from_str(&content) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            log::warn!("kumo: ignoring invalid config.toml: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Flat `key = value` parser (Ghostty-style)
// ---------------------------------------------------------------------------

/// Parse a Ghostty-style flat config file into a key/value map. Blank lines
/// and `#` comments are ignored; values may be single- or double-quoted.
fn parse_flat(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let content = std::fs::read_to_string(path)?;
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            let key = key.trim();
            if !key.is_empty() {
                map.insert(key.to_string(), unquote(value.trim()).to_string());
            }
        }
    }
    Ok(map)
}

/// Strip a matching pair of surrounding quotes from a value.
fn unquote(v: &str) -> &str {
    let bytes = v.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &v[1..v.len() - 1];
        }
    }
    v
}

// ---------------------------------------------------------------------------
// High-level accessors
// ---------------------------------------------------------------------------

/// Resolve the AI CLI command line to run inside the AI pane.
///
/// Precedence: the `config` file (`ai-cmd`), then the legacy `~/.kumo` file
/// (`ai_cmd`), then a built-in default of `opencode`.
pub fn ai_command() -> (String, Vec<String>) {
    let cfg = cached_config();
    cfg.ai_cmd.unwrap_or_else(|| (String::from("opencode"), Vec::new()))
}

/// Working directory for the AI pane. Prefers the persisted workspace
/// (`~/.local/state/kumo/workspace.json`, falling back to the legacy
/// `~/.kumo/workspace.json`) so opencode runs inside the project and
/// `@file` references resolve; falls back to `$HOME`.
pub fn ai_cwd() -> PathBuf {
    let mut candidates = vec![state_dir().join("workspace.json")];
    if let Some(legacy) = legacy_workspace_file() {
        candidates.push(legacy);
    }
    for wp in &candidates {
        if let Ok(ws) = std::fs::read_to_string(wp) {
            let ws = ws.trim();
            if !ws.is_empty() {
                let pb = PathBuf::from(ws);
                if pb.is_dir() {
                    return pb;
                }
            }
        }
    }
    home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// The user's login shell: the `shell` config key, else `$SHELL`, else bash.
pub fn default_shell() -> String {
    let cfg = cached_config();
    cfg.shell
        .or_else(|| env_nonempty("SHELL"))
        .unwrap_or_else(|| "/bin/bash".to_string())
}

/// Whether the startup update check is enabled. Disabled by `update-check =
/// false` in the config file or by setting `KUMO_NO_UPDATE=1`.
pub fn update_check_enabled() -> bool {
    if std::env::var("KUMO_NO_UPDATE").is_ok() {
        return false;
    }
    cached_config().update_check
}

/// Agent-notification policy for lifecycle transitions (`[notifications]`).
/// Toasts silenced entirely by setting `[notifications] position = "off"` or
/// `KUMO_NO_NOTIFY=1`. Read live on every use, so `kumo reload` applies
/// changes without a restart.
pub fn agent_notifications() -> NotificationsConfig {
    if std::env::var("KUMO_NO_NOTIFY").is_ok() {
        return NotificationsConfig { position: ToastPosition::Off, ..Default::default() };
    }
    cached_config().notifications
}

/// Whether agent lifecycle transitions play an audible chime. Disabled by
/// `[notifications] sound = false`, the deprecated top-level
/// `agent-sound = false`, or by setting `KUMO_NO_SOUND=1`.
pub fn agent_sound_enabled() -> bool {
    if std::env::var("KUMO_NO_SOUND").is_ok() {
        return false;
    }
    cached_config().notifications.sound
}

/// Where attached viewers anchor the agent-lifecycle toast stack
/// (`[notifications] position`, top-right by default).
pub fn toast_position() -> ToastPosition {
    cached_config().notifications.position
}

/// The leader chord from the config (`leader = "ctrl+b"`), if set. `None`
/// means the built-in default (Ctrl+B).
pub fn leader() -> Option<String> {
    cached_config().leader
}

/// `[keymap.bindings]` overrides: chord string → action id. Empty when the
/// config sets no bindings (the stock keymap applies).
pub fn keymap_bindings() -> HashMap<String, String> {
    cached_config().keymap_bindings
}

/// The resolved session working-directory policy (`[terminal] new-cwd`):
/// `Follow` (default), `Home`, `Current`, or `Fixed(path)`. A `Fixed` mode
/// without a usable `fixed-cwd` was already normalized to `Current` at load.
pub fn new_cwd() -> NewCwd {
    let cfg = cached_config();
    match cfg.new_cwd {
        NewCwdMode::Follow => NewCwd::Follow,
        NewCwdMode::Current => NewCwd::Current,
        NewCwdMode::Home => NewCwd::Home,
        NewCwdMode::Fixed => match cfg.fixed_cwd {
            Some(p) if p.is_dir() => NewCwd::Fixed(p),
            _ => NewCwd::Current,
        },
    }
}

/// Selected theme name from `config.toml` (`[theme] name = "..."` or flat `theme = "..."`).
/// `None` means the built-in default.
pub fn theme_name() -> Option<String> {
    cached_config().theme
}

/// Custom theme defined in `[theme.custom]` if present.
pub fn custom_theme() -> Option<crate::theme::OwnedTheme> {
    cached_config().custom_theme
}

/// All themes including the optional custom theme at the end.
pub fn all_themes() -> Vec<crate::theme::OwnedTheme> {
    let cfg = cached_config();
    crate::theme::all_themes(cfg.custom_theme)
}

/// Resolve the initial theme index, respecting `theme = "..."` and whether a
/// custom theme exists.
pub fn theme_index() -> usize {
    let cfg = cached_config();
    crate::theme::resolve_theme_idx(cfg.theme.as_deref(), cfg.custom_theme.as_ref())
}

/// Sidebar configuration (`[sidebar]`). Clone used by clients to read order/
/// visibility/border style.
pub fn sidebar() -> SidebarConfig {
    cached_config().sidebar
}

/// Sidebar border style.
pub fn sidebar_borders() -> SidebarBorders {
    cached_config().sidebar.borders
}

/// Whether a sidebar section is visible (sessions/agents).
pub fn sidebar_sections() -> SidebarSections {
    cached_config().sidebar.sections
}

/// Ordered sidebar sections permutation.
pub fn sidebar_order() -> Vec<SidebarSection> {
    cached_config().sidebar.order
}

/// Status bar configuration (`[status_bar]`).
pub fn status_bar() -> StatusBarConfig {
    cached_config().status_bar
}

/// Whether the status bar is enabled.
pub fn status_bar_enabled() -> bool {
    cached_config().status_bar.enabled
}

/// Worktree shared-dirs (gitignored symlinks/clone-copies).
pub fn worktree_shared_dirs() -> Vec<PathBuf> {
    cached_config().worktree.shared_dirs
}

/// Whether spawned panes receive `KUMO_SOCKET_PATH`/`KUMO_BIN_PATH`.
pub fn worktree_expose_socket() -> bool {
    cached_config().worktree.expose_socket
}

/// Whether the checkpoint skill is auto-installed (default: true).
pub fn checkpoints_enabled() -> bool {
    cached_config().checkpoints.enabled
}

/// Split a command line string into program + args (space separated).
fn split_cmd(raw: &str) -> (String, Vec<String>) {
    let mut it = raw.split_whitespace();
    let program = it.next().unwrap_or("opencode").to_string();
    let args: Vec<String> = it.map(|s| s.to_string()).collect();
    (program, args)
}

/// Read a non-empty env var.
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}

/// `$HOME`.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

static RESOLVE_CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Option<String>>>> =
    std::sync::OnceLock::new();

fn resolve_cache() -> &'static std::sync::Mutex<HashMap<String, Option<String>>> {
    RESOLVE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Resolve a program name to an absolute path. GUI-launched apps (Spotlight /
/// Finder) get a minimal `PATH`, so `opencode` would otherwise fail to spawn;
/// the login shell is asked for its PATH as a fallback. Falls back to the
/// original program when it cannot be resolved. Results are cached.
pub fn resolve_program(program: &str) -> String {
    if program.is_empty() {
        return program.to_string();
    }
    if let Some(cached) = resolve_cache().lock().unwrap().get(program) {
        return cached.clone().unwrap_or_else(|| program.to_string());
    }
    let resolved = resolve_program_uncached(program);
    let result = resolved.clone().unwrap_or_else(|| program.to_string());
    resolve_cache().lock().unwrap().insert(program.to_string(), resolved);
    result
}

fn resolve_program_uncached(program: &str) -> Option<String> {
    let pb = PathBuf::from(program);
    if pb.is_absolute() {
        return pb.is_file().then(|| program.to_string());
    }
    if let Some(found) = which_in_path(program) {
        return Some(found);
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let out = std::process::Command::new(&shell)
        .arg("-l")
        .arg("-c")
        .arg(format!("command -v {}", program))
        .output()
        .ok()?;
    if out.status.success() {
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let pb = PathBuf::from(&path);
        if pb.is_absolute() && pb.is_file() {
            return Some(path);
        }
    }
    None
}

fn which_in_path(program: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(dir).join(program);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// Serializes process-wide env mutation between the config tests and any other
/// test (e.g. the daemon integration test) that must override config/env.
/// Always compiled so test targets in dependent crates can lock it too.
pub static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Restore env vars on drop so tests never leak mutations.
    struct EnvGuard(Vec<(&'static str, Option<String>)>);

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, value);
            EnvGuard(vec![(key, prev)])
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            EnvGuard(vec![(key, prev)])
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    /// Create a unique scratch dir (std-only; no tempfile dependency).
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kumo-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &PathBuf, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn config_dir_uses_xdg_when_set() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let xdg = scratch_dir("xdg");
        let _guards = (
            EnvGuard::set("XDG_CONFIG_HOME", &xdg.to_string_lossy()),
            EnvGuard::unset("KUMO_CONFIG_DIR"),
            EnvGuard::set("HOME", "/tmp/nonexistent-home-xdg"),
        );
        assert_eq!(config_dir(), xdg.join("kumo"));
    }

    #[test]
    fn config_dir_overridden_by_env() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let custom = scratch_dir("kumo-custom");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &custom.to_string_lossy()),
            EnvGuard::unset("XDG_CONFIG_HOME"),
            EnvGuard::set("HOME", "/tmp/nonexistent-home-custom"),
        );
        assert_eq!(config_dir(), custom);
    }

    #[test]
    fn config_dir_falls_back_to_home() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let home = scratch_dir("home");
        let _guards = (
            EnvGuard::unset("KUMO_CONFIG_DIR"),
            EnvGuard::unset("XDG_CONFIG_HOME"),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(config_dir(), home.join(".config").join("kumo"));
    }

    #[test]
    fn state_dir_uses_xdg_when_set() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let xdg = scratch_dir("xdg-state");
        let _guards = (
            EnvGuard::set("XDG_STATE_HOME", &xdg.to_string_lossy()),
            EnvGuard::unset("KUMO_STATE_DIR"),
            EnvGuard::set("HOME", "/tmp/nonexistent-home-state"),
        );
        assert_eq!(state_dir(), xdg.join("kumo"));
    }

    #[test]
    fn state_dir_falls_back_to_home() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let home = scratch_dir("home-state");
        let _guards = (
            EnvGuard::unset("KUMO_STATE_DIR"),
            EnvGuard::unset("XDG_STATE_HOME"),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(state_dir(), home.join(".local").join("state").join("kumo"));
    }

    #[test]
    fn runtime_dir_prefers_xdg_runtime() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let _guards = (
            EnvGuard::set("XDG_RUNTIME_DIR", "/run/user/1000"),
            EnvGuard::unset("TMPDIR"),
        );
        assert_eq!(runtime_dir(), PathBuf::from("/run/user/1000/kumo"));
    }

    #[test]
    fn runtime_dir_falls_back_to_tmp() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let _guards = (
            EnvGuard::unset("XDG_RUNTIME_DIR"),
            EnvGuard::set("TMPDIR", "/var/folders/xyz"),
        );
        assert_eq!(runtime_dir(), PathBuf::from("/var/folders/xyz/kumo"));
    }

    #[test]
    fn default_is_opencode() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-empty");
        let home = scratch_dir("home-empty");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let (prog, args) = ai_command();
        assert_eq!(prog, "opencode");
        assert!(args.is_empty());
    }

    #[test]
    fn ai_command_reads_config_file() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-ai");
        let home = scratch_dir("home-ai");
        write(&cfg_dir.join("config"), "ai-cmd = claude --model sonnet\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let (prog, args) = ai_command();
        assert_eq!(prog, "claude");
        assert_eq!(args, vec!["--model", "sonnet"]);
    }

    #[test]
    fn ai_command_accepts_underscore_alias() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-ai-underscore");
        let home = scratch_dir("home-ai-underscore");
        write(&cfg_dir.join("config"), "ai_cmd = codex\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let (prog, args) = ai_command();
        assert_eq!(prog, "codex");
        assert!(args.is_empty());
    }

    #[test]
    fn ai_command_reads_legacy_kumo_file() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-legacy");
        let home = scratch_dir("home-legacy");
        write(&home.join(".kumo"), "ai_cmd = gemini\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let (prog, _) = ai_command();
        assert_eq!(prog, "gemini");
    }

    #[test]
    fn ai_command_config_wins_over_legacy() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-wins");
        let home = scratch_dir("home-wins");
        write(&cfg_dir.join("config"), "ai-cmd = claude\n");
        write(&home.join(".kumo"), "ai_cmd = codex\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let (prog, _) = ai_command();
        assert_eq!(prog, "claude");
    }

    #[test]
    fn agent_sound_defaults_on_and_parses_off() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-sound");
        let home = scratch_dir("home-sound");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::unset("KUMO_NO_SOUND"),
        );
        assert!(agent_sound_enabled(), "the chime should default to on");
        write(&cfg_dir.join("config"), "agent-sound = false\n");
        assert!(
            !agent_sound_enabled(),
            "flat agent-sound = false must disable the chime (deprecated alias)"
        );
    }

    #[test]
    fn notifications_sound_parses_and_section_wins_over_deprecated_alias() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-notify-sound");
        let home = scratch_dir("home-notify-sound");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::unset("KUMO_NO_SOUND"),
        );
        write(&cfg_dir.join("config.toml"), "[notifications]\nsound = false\n");
        assert!(!agent_sound_enabled(), "[notifications] sound must gate the chime");

        // The alert-sound spellings are aliases of `sound`.
        write(&cfg_dir.join("config.toml"), "[notifications]\nalert_sound = false\n");
        assert!(!agent_sound_enabled(), "alert_sound is an alias of sound");
        write(&cfg_dir.join("config.toml"), "[notifications]\nalert-sound = false\n");
        assert!(!agent_sound_enabled(), "alert-sound is an alias of sound");

        // Precedence: deprecated top-level `agent-sound` loses to the section.
        write(
            &cfg_dir.join("config.toml"),
            "agent-sound = false\n\n[notifications]\nsound = true\n",
        );
        assert!(
            agent_sound_enabled(),
            "[notifications] sound must win over the deprecated top-level alias"
        );

        // Without a section value the top-level alias still applies.
        write(&cfg_dir.join("config.toml"), "agent-sound = false\n");
        assert!(!agent_sound_enabled());
        write(&cfg_dir.join("config.toml"), "");
        assert!(agent_sound_enabled(), "back to default with an empty config");
    }

    #[test]
    fn agent_sound_disabled_by_env() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-sound-env");
        let home = scratch_dir("home-sound-env");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::set("KUMO_NO_SOUND", "1"),
        );
        assert!(!agent_sound_enabled(), "KUMO_NO_SOUND must disable alerts");
    }

    #[test]
    fn notifications_toasts_on_by_default_and_off_via_position() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-notify");
        let home = scratch_dir("home-notify");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::unset("KUMO_NO_NOTIFY"),
        );
        let cfg = agent_notifications();
        assert!(cfg.toasts_enabled(), "toasts default on at the top-right corner");
        assert_eq!(cfg.position, ToastPosition::TopRight);
        assert!(cfg.blocked && cfg.finished, "channel switches default on");

        write(&cfg_dir.join("config.toml"), "[notifications]\nposition = \"off\"\n");
        let cfg = agent_notifications();
        assert!(!cfg.toasts_enabled(), "position = \"off\" must silence toasts");
        assert!(cfg.sound, "the chime is independent of position");

        write(&cfg_dir.join("config.toml"), "[notifications]\nposition = \"never\"\nblocked = false\nidle = false\n");
        let cfg = agent_notifications();
        assert!(!cfg.toasts_enabled(), "never is an alias of off");
        assert!(!cfg.blocked, "blocked channel must be disableable");
        assert!(!cfg.finished, "the idle alias must feed finished");

        write(&cfg_dir.join("config.toml"), "[notifications]\nposition = \"bottom-left\"\n");
        assert!(
            agent_notifications().toasts_enabled(),
            "any real position re-enables toasts"
        );

        write(&cfg_dir.join("config.toml"), "[notifications]\nenabled = true\n");
        assert_eq!(
            agent_notifications().position,
            ToastPosition::TopRight,
            "`enabled` no longer exists; unknown keys are ignored"
        );
    }

    #[test]
    fn notifications_disabled_by_env() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-notify-env");
        let home = scratch_dir("home-notify-env");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::set("KUMO_NO_NOTIFY", "1"),
        );
        assert!(
            !agent_notifications().toasts_enabled(),
            "KUMO_NO_NOTIFY must disable toasts"
        );
    }

    #[test]
    fn split_cmd_handles_plain_program() {
        let (prog, args) = split_cmd("opencode");
        assert_eq!(prog, "opencode");
        assert!(args.is_empty());
    }

    #[test]
    fn default_shell_uses_config_when_set() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-shell");
        let home = scratch_dir("home-shell");
        write(&cfg_dir.join("config"), "shell = /bin/bash\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::set("SHELL", "/bin/fish"),
        );
        assert_eq!(default_shell(), "/bin/bash");
    }

    #[test]
    fn default_shell_falls_back_to_env() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-shell-empty");
        let home = scratch_dir("home-shell-empty");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::set("SHELL", "/bin/fish"),
        );
        assert_eq!(default_shell(), "/bin/fish");
    }

    #[test]
    fn ai_cwd_prefers_state_dir() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let state = scratch_dir("state-cwd");
        let home = scratch_dir("home-cwd");
        let project = scratch_dir("project-cwd");
        write(&state.join("workspace.json"), &project.to_string_lossy());
        let _guards = (
            EnvGuard::set("KUMO_STATE_DIR", &state.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(ai_cwd(), project);
    }

    #[test]
    fn ai_cwd_falls_back_to_legacy_workspace() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let state = scratch_dir("state-cwd-legacy");
        let home = scratch_dir("home-cwd-legacy");
        let project = scratch_dir("project-cwd-legacy");
        write(&home.join(".kumo").join("workspace.json"), &project.to_string_lossy());
        let _guards = (
            EnvGuard::set("KUMO_STATE_DIR", &state.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(ai_cwd(), project);
    }

    #[test]
    fn ai_cwd_falls_back_to_home() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let state = scratch_dir("state-cwd-none");
        let home = scratch_dir("home-cwd-none");
        let _guards = (
            EnvGuard::set("KUMO_STATE_DIR", &state.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(ai_cwd(), home);
    }

    #[test]
    fn resolve_program_keeps_existing_absolute_paths() {
        let abs = std::env::current_dir().unwrap().join("Cargo.toml");
        let abs_str = abs.to_string_lossy().to_string();
        assert_eq!(resolve_program(&abs_str), abs_str);
    }

    #[test]
    fn resolve_program_falls_back_when_not_found() {
        let name = "definitely-not-a-real-cmd-xyz-123";
        assert_eq!(resolve_program(name), name);
    }

    #[test]
    fn toml_parses_native_types() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-toml");
        let home = scratch_dir("home-toml");
        write(
            &cfg_dir.join("config.toml"),
            "ai-cmd = \"claude --model sonnet\"\nshell = \"/opt/homebrew/bin/fish\"\nupdate-check = false\nagent-sound = false\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::unset("KUMO_NO_UPDATE"),
            EnvGuard::unset("KUMO_NO_SOUND"),
        );
        let (prog, args) = ai_command();
        assert_eq!(prog, "claude");
        assert_eq!(args, vec!["--model", "sonnet"]);
        assert_eq!(default_shell(), "/opt/homebrew/bin/fish");
        assert!(!update_check_enabled());
        assert!(!agent_sound_enabled());
    }

    #[test]
    fn toml_wins_over_flat_config() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-toml-wins");
        let home = scratch_dir("home-toml-wins");
        write(&cfg_dir.join("config"), "ai-cmd = codex\nshell = /bin/bash\n");
        write(&cfg_dir.join("config.toml"), "ai-cmd = \"claude\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let (prog, _) = ai_command();
        assert_eq!(prog, "claude", "TOML must override the flat file");
        assert_eq!(default_shell(), "/bin/bash", "unset TOML keys fall back to flat");
    }

    #[test]
    fn toml_accepts_ai_cmd_alias() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-toml-alias");
        let home = scratch_dir("home-toml-alias");
        write(&cfg_dir.join("config.toml"), "ai_cmd = \"gemini\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let (prog, _) = ai_command();
        assert_eq!(prog, "gemini");
    }

    #[test]
    fn invalid_toml_falls_back_to_flat() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-toml-bad");
        let home = scratch_dir("home-toml-bad");
        write(&cfg_dir.join("config"), "ai-cmd = codex\n");
        write(&cfg_dir.join("config.toml"), "shell = /unquoted/path\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let (prog, _) = ai_command();
        assert_eq!(prog, "codex", "unparsable TOML must fall back to the flat file");
    }

    #[test]
    fn leader_reads_from_keymap_table() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-leader");
        let home = scratch_dir("home-leader");
        write(&cfg_dir.join("config.toml"), "[keymap]\nleader = \"ctrl+space\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(leader().as_deref(), Some("ctrl+space"));
    }

    #[test]
    fn leader_keymap_wins_over_top_level_alias() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-leader-wins");
        let home = scratch_dir("home-leader-wins");
        write(
            &cfg_dir.join("config.toml"),
            "leader = \"f12\"\n[keymap]\nleader = \"ctrl+space\"\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(leader().as_deref(), Some("ctrl+space"), "[keymap].leader must win");
    }

    #[test]
    fn leader_reads_top_level_alias_as_fallback() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-leader-alias");
        let home = scratch_dir("home-leader-alias");
        write(&cfg_dir.join("config.toml"), "leader = \"f12\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(leader().as_deref(), Some("f12"));
    }

    #[test]
    fn keymap_bindings_reads_from_toml() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-keymap");
        let home = scratch_dir("home-keymap");
        write(
            &cfg_dir.join("config.toml"),
            "[keymap]\nleader = \"ctrl+b\"\n[keymap.bindings]\ns = \"split-vertical\"\nv = \"close-pane\"\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let bindings = keymap_bindings();
        assert_eq!(bindings.get("s").map(String::as_str), Some("split-vertical"));
        assert_eq!(bindings.get("v").map(String::as_str), Some("close-pane"));
        assert_eq!(bindings.len(), 2);
    }

    #[test]
    fn keymap_bindings_default_to_empty() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-keymap-empty");
        let home = scratch_dir("home-keymap-empty");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert!(keymap_bindings().is_empty(), "no bindings table means the stock keymap");
    }

    #[test]
    fn leader_defaults_to_none() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-leader-none");
        let home = scratch_dir("home-leader-none");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(leader(), None, "no leader key means the built-in default");
    }

    #[test]
    fn missing_toml_uses_flat_only() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-no-toml");
        let home = scratch_dir("home-no-toml");
        write(&cfg_dir.join("config"), "ai-cmd = opencode\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let (prog, _) = ai_command();
        assert_eq!(prog, "opencode");
    }

    #[test]
    fn new_cwd_defaults_to_follow() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-nc-default");
        let home = scratch_dir("home-nc-default");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(new_cwd(), NewCwd::Follow, "new-cwd should default to follow");
    }

    #[test]
    fn new_cwd_parses_flat_values() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let home = scratch_dir("home-nc-flat");
        let cases = [
            ("new-cwd = home\n", NewCwd::Home),
            ("new-cwd = current\n", NewCwd::Current),
            ("new-cwd = follow\n", NewCwd::Follow),
        ];
        for (i, (body, expect)) in cases.iter().enumerate() {
            let cfg_dir = scratch_dir(&format!("cfg-nc-flat-{i}"));
            write(&cfg_dir.join("config"), body);
            let _guards = (
                EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
                EnvGuard::set("HOME", &home.to_string_lossy()),
            );
            assert_eq!(&new_cwd(), expect, "flat {body:?}");
        }
    }

    #[test]
    fn new_cwd_fixed_uses_fixed_cwd_path() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-nc-fixed");
        let home = scratch_dir("home-nc-fixed");
        let project = scratch_dir("project-nc-fixed");
        write(
            &cfg_dir.join("config.toml"),
            &format!("[terminal]\nnew-cwd = \"fixed\"\nfixed-cwd = \"{}\"\n", project.display()),
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(new_cwd(), NewCwd::Fixed(project));
    }

    #[test]
    fn new_cwd_fixed_without_path_falls_back_to_current() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-nc-fixed-none");
        let home = scratch_dir("home-nc-fixed-none");
        write(&cfg_dir.join("config"), "new-cwd = fixed\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(new_cwd(), NewCwd::Current, "fixed without fixed-cwd must fall back to current");
    }

    #[test]
    fn new_cwd_invalid_value_falls_back_to_follow() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-nc-bad");
        let home = scratch_dir("home-nc-bad");
        write(&cfg_dir.join("config.toml"), "[terminal]\nnew-cwd = \"banana\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(new_cwd(), NewCwd::Follow, "invalid new-cwd must fall back to follow");
    }

    #[test]
    fn terminal_shell_wins_over_top_level_alias() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-term-shell");
        let home = scratch_dir("home-term-shell");
        write(
            &cfg_dir.join("config.toml"),
            "shell = \"/bin/bash\"\n[terminal]\nshell = \"/bin/fish\"\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::set("SHELL", "/bin/zsh"),
        );
        assert_eq!(default_shell(), "/bin/fish", "[terminal] shell must win over the top-level alias");
    }

    #[test]
    fn top_level_shell_alias_still_read() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-shell-alias");
        let home = scratch_dir("home-shell-alias");
        write(&cfg_dir.join("config.toml"), "shell = \"/bin/bash\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::set("SHELL", "/bin/zsh"),
        );
        assert_eq!(default_shell(), "/bin/bash", "deprecated top-level shell must keep working");
    }

    #[test]
    fn theme_name_reads_from_table() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-theme-name");
        let home = scratch_dir("home-theme-name");
        write(&cfg_dir.join("config.toml"), "[theme]\nname = \"dracula\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(theme_name().as_deref(), Some("dracula"));
        assert_eq!(theme_index(), 6, "dracula should be index 6");
    }

    #[test]
    fn theme_name_case_insensitive() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-theme-case");
        let home = scratch_dir("home-theme-case");
        write(&cfg_dir.join("config.toml"), "[theme]\nname = \"Tokyo Night\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(theme_index(), 7);
    }

    #[test]
    fn custom_theme_parses_palette_and_chrome() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-custom");
        let home = scratch_dir("home-custom");
        write(
            &cfg_dir.join("config.toml"),
            "[theme]\nname = \"custom\"\n[theme.custom]\nname = \"MyCustom\"\npalette = [\"#ff0000\", \"#00ff00\", \"#0000ff\"]\naccent = \"#123456\"\nterm_bg = \"#111111\"\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let ct = custom_theme().expect("custom theme should be parsed");
        assert_eq!(ct.name, "MyCustom");
        assert_eq!(ct.palette[0], crate::color::ColorRgb::new(0xff, 0x00, 0x00));
        assert_eq!(ct.palette[1], crate::color::ColorRgb::new(0x00, 0xff, 0x00));
        assert_eq!(ct.accent, ratatui::style::Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(ct.term_bg, crate::color::ColorRgb::new(0x11, 0x11, 0x11));
        assert_eq!(theme_name().as_deref(), Some("custom"));
        // custom is at THEMES.len() == 8
        assert_eq!(theme_index(), crate::theme::THEMES.len());
        assert_eq!(all_themes().len(), crate::theme::THEMES.len() + 1);
    }

    #[test]
    fn custom_theme_invalid_hex_is_warned_and_ignored() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-custom-bad");
        let home = scratch_dir("home-custom-bad");
        write(
            &cfg_dir.join("config.toml"),
            "[theme.custom]\naccent = \"not-a-color\"\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let ct = custom_theme().expect("even with bad accent, custom should exist (fallback)");
        // accent should remain default (spider-verse accent #ef3945)
        assert_eq!(ct.accent, ratatui::style::Color::Rgb(0xef, 0x39, 0x45));
    }

    #[test]
    fn theme_flat_key_parses() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-theme-flat");
        let home = scratch_dir("home-theme-flat");
        write(&cfg_dir.join("config"), "theme = gruvbox\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(theme_name().as_deref(), Some("gruvbox"));
        assert_eq!(theme_index(), 5);
    }

    #[test]
    fn sidebar_defaults_to_sessions_agents_and_rounded() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-sidebar-default");
        let home = scratch_dir("home-sidebar-default");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let s = sidebar();
        assert_eq!(s.order, vec![SidebarSection::Sessions, SidebarSection::Agents]);
        assert!(s.sections.sessions && s.sections.agents);
        assert_eq!(s.borders.style, BorderStyle::Rounded);
        assert_eq!(s.layout, SidebarLayout::Project);
    }

    #[test]
    fn sidebar_parses_layout() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-sidebar-layout");
        let home = scratch_dir("home-sidebar-layout");
        write(&cfg_dir.join("config.toml"), "[sidebar]\nlayout = \"tabs\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(sidebar().layout, SidebarLayout::Tabs);
        // Unknown values fall back to the default.
        write(&cfg_dir.join("config.toml"), "[sidebar]\nlayout = \"fancy\"\n");
        assert_eq!(sidebar().layout, SidebarLayout::Project);
    }

    #[test]
    fn sidebar_parses_order_and_visibility() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-sidebar-order");
        let home = scratch_dir("home-sidebar-order");
        write(
            &cfg_dir.join("config.toml"),
            "[sidebar]\norder = [\"agents\", \"sessions\"]\n[sidebar.sections]\nsessions = true\nagents = false\n[sidebar.borders]\nstyle = \"hidden\"\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let s = sidebar();
        assert_eq!(s.order, vec![SidebarSection::Agents, SidebarSection::Sessions]);
        // config normalizes both-hidden to at least sessions visible, but here agents false only
        assert!(s.sections.sessions);
        assert!(!s.sections.agents);
        assert_eq!(s.borders.style, BorderStyle::Hidden);
    }

    #[test]
    fn sidebar_invalid_style_falls_back_to_rounded() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-sidebar-bad-style");
        let home = scratch_dir("home-sidebar-bad-style");
        write(&cfg_dir.join("config.toml"), "[sidebar.borders]\nstyle = \"banana\"\n");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        assert_eq!(sidebar_borders().style, BorderStyle::Rounded, "invalid style must fallback to rounded");
    }

    #[test]
    fn sidebar_order_ignores_unknown_and_dedupes() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-sidebar-order-unk");
        let home = scratch_dir("home-sidebar-order-unk");
        write(
            &cfg_dir.join("config.toml"),
            "[sidebar]\norder = [\"sessions\", \"unknown\", \"sessions\", \"agents\"]\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let s = sidebar();
        assert_eq!(s.order, vec![SidebarSection::Sessions, SidebarSection::Agents], "unknown ignored, dupes removed");
    }

    #[test]
    fn border_style_parses_all_variants() {
        assert_eq!(BorderStyle::parse("single"), Some(BorderStyle::Single));
        assert_eq!(BorderStyle::parse("rounded"), Some(BorderStyle::Rounded));
        assert_eq!(BorderStyle::parse("double"), Some(BorderStyle::Double));
        assert_eq!(BorderStyle::parse("heavy"), Some(BorderStyle::Heavy));
        assert_eq!(BorderStyle::parse("hidden"), Some(BorderStyle::Hidden));
        assert_eq!(BorderStyle::parse("none"), Some(BorderStyle::Hidden));
        assert_eq!(BorderStyle::parse("unknown"), None);
    }

    #[test]
    fn toast_position_parses_all_variants() {
        assert_eq!(ToastPosition::parse("top-right"), Some(ToastPosition::TopRight));
        assert_eq!(ToastPosition::parse("Top_Right"), Some(ToastPosition::TopRight));
        assert_eq!(ToastPosition::parse("top-left"), Some(ToastPosition::TopLeft));
        assert_eq!(ToastPosition::parse("bottom-right"), Some(ToastPosition::BottomRight));
        assert_eq!(ToastPosition::parse("BOTTOMLEFT"), Some(ToastPosition::BottomLeft));
        assert_eq!(ToastPosition::parse(" centre "), Some(ToastPosition::Center));
        assert_eq!(ToastPosition::parse("centre"), Some(ToastPosition::Center));
        assert_eq!(ToastPosition::parse("never"), Some(ToastPosition::Off));
        assert_eq!(ToastPosition::parse("off"), Some(ToastPosition::Off));
        assert_eq!(ToastPosition::parse("none"), Some(ToastPosition::Off));
        assert_eq!(ToastPosition::parse("disabled"), Some(ToastPosition::Off));
        assert_eq!(ToastPosition::parse("banana"), None);
        assert_eq!(ToastPosition::default(), ToastPosition::TopRight);
    }

    #[test]
    fn notifications_position_defaults_top_right_and_parses_from_toml() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-notify-position");
        let home = scratch_dir("home-notify-position");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::unset("KUMO_NO_NOTIFY"),
        );
        assert_eq!(
            agent_notifications().position,
            ToastPosition::TopRight,
            "toasts default to the top-right corner"
        );

        write(&cfg_dir.join("config.toml"), "[notifications]\nposition = \"bottom-left\"\n");
        assert_eq!(agent_notifications().position, ToastPosition::BottomLeft);

        write(&cfg_dir.join("config.toml"), "[notifications]\nposition = \"centre\"\n");
        assert_eq!(agent_notifications().position, ToastPosition::Center);

        write(&cfg_dir.join("config.toml"), "[notifications]\nposition = \"banana\"\n");
        assert_eq!(
            agent_notifications().position,
            ToastPosition::TopRight,
            "invalid position must fall back to top-right"
        );
    }

    #[test]
    fn status_bar_defaults() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-status-default");
        let home = scratch_dir("home-status-default");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let s = status_bar();
        assert!(s.enabled);
        assert_eq!(s.left, vec![StatusWidget::Mode, StatusWidget::Menu, StatusWidget::Session]);
        assert_eq!(s.center, vec![StatusWidget::Branch]);
        assert_eq!(s.right, vec![StatusWidget::AgentStatus, StatusWidget::Clock]);
        assert_eq!(s.widgets.clock.format, "%H:%M");
        assert!(s.widgets.branch.show_ahead_behind);
        assert_eq!(s.widgets.branch.max_len, 20);
        assert_eq!(s.widgets.agent.style, AgentWidgetStyle::Counts);
    }

    #[test]
    fn status_bar_parses_layout_and_opts() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-status-layout");
        let home = scratch_dir("home-status-layout");
        write(
            &cfg_dir.join("config.toml"),
            "[status_bar]\nenabled = false\nleft = [\"session\", \"branch\"]\ncenter = [\"clock\"]\nright = [\"hostname\", \"agent_status\"]\n[status_bar.widgets.clock]\nformat = \"%H:%M:%S\"\n[status_bar.widgets.branch]\nshow_ahead_behind = false\nmax_len = 10\n[status_bar.widgets.agent]\nstyle = \"dots\"\nonly_blocked = true\n[status_bar.widgets.hostname]\nstyle = \"fqdn\"\nshow_user = true\nonly_ssh = true\n[status_bar.widgets.session]\nshow_tabs = false\nshow_panes = false\nshow_zoom = false\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let s = status_bar();
        assert!(!s.enabled);
        assert_eq!(s.left, vec![StatusWidget::Session, StatusWidget::Branch]);
        assert_eq!(s.center, vec![StatusWidget::Clock]);
        assert_eq!(s.right, vec![StatusWidget::Hostname, StatusWidget::AgentStatus]);
        assert_eq!(s.widgets.clock.format, "%H:%M:%S");
        assert!(!s.widgets.branch.show_ahead_behind);
        assert_eq!(s.widgets.branch.max_len, 10);
        assert_eq!(s.widgets.agent.style, AgentWidgetStyle::Dots);
        assert!(s.widgets.agent.only_blocked);
        assert_eq!(s.widgets.hostname.style, HostnameStyle::Fqdn);
        assert!(s.widgets.hostname.show_user);
        assert!(s.widgets.hostname.only_ssh);
        assert!(!s.widgets.session.show_tabs);
        assert!(!s.widgets.session.show_panes);
        assert!(!s.widgets.session.show_zoom);
    }

    #[test]
    fn status_bar_ignores_unknown_widgets_and_dedupes() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-status-unk");
        let home = scratch_dir("home-status-unk");
        write(
            &cfg_dir.join("config.toml"),
            "[status_bar]\nleft = [\"session\", \"unknown\", \"session\", \"branch\"]\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let s = status_bar();
        assert_eq!(s.left, vec![StatusWidget::Session, StatusWidget::Branch], "unknown ignored, dupes removed");
        // right stays default when not set
        assert_eq!(s.right, vec![StatusWidget::AgentStatus, StatusWidget::Clock]);
    }

    #[test]
    fn status_bar_alias_with_dash_parses() {
        let _g = TEST_ENV_LOCK.lock().unwrap();
        let cfg_dir = scratch_dir("cfg-status-dash");
        let home = scratch_dir("home-status-dash");
        write(
            &cfg_dir.join("config.toml"),
            "[status-bar]\nleft = [\"clock\"]\n",
        );
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg_dir.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
        );
        let s = status_bar();
        assert_eq!(s.left, vec![StatusWidget::Clock]);
    }

    #[test]
    fn status_widget_parse_variants() {
        assert_eq!(StatusWidget::parse("mode"), Some(StatusWidget::Mode));
        assert_eq!(StatusWidget::parse("MENU"), Some(StatusWidget::Menu));
        assert_eq!(StatusWidget::parse("agent_status"), Some(StatusWidget::AgentStatus));
        assert_eq!(StatusWidget::parse("agent-status"), Some(StatusWidget::AgentStatus));
        assert_eq!(StatusWidget::parse("host"), Some(StatusWidget::Hostname));
        assert_eq!(StatusWidget::parse("time"), Some(StatusWidget::Clock));
        assert_eq!(StatusWidget::parse("unknown"), None);
    }
}
