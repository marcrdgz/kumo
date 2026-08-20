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
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            order: vec![SidebarSection::Sessions, SidebarSection::Agents],
            sections: SidebarSections::default(),
            borders: SidebarBorders::default(),
        }
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
    /// Whether agent lifecycle transitions (blocked / finished) play a sound
    /// (default: true).
    pub agent_sound: bool,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ai_cmd: None,
            shell: None,
            update_check: true,
            agent_sound: true,
            leader: None,
            keymap_bindings: HashMap::new(),
            new_cwd: NewCwdMode::default(),
            fixed_cwd: None,
            theme: None,
            custom_theme: None,
            sidebar: SidebarConfig::default(),
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
        // Legacy flat keys for borders style
        // handled in from_map; toml is canonical.
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
        cfg.agent_sound = map
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
        if let Some(v) = toml.agent_sound {
            self.agent_sound = v;
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
        self.normalize_new_cwd();
    }
}

/// Raw TOML for `[sidebar]` — all optional, unknown keys ignored.
#[derive(Default, serde::Deserialize, Debug)]
pub struct SidebarSectionRaw {
    pub order: Option<Vec<String>>,
    pub sections: Option<SidebarSectionsRaw>,
    pub borders: Option<SidebarBordersRaw>,
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

/// Whether agent lifecycle transitions play an audible alert. Disabled by
/// `agent-sound = false` in the config file or by setting `KUMO_NO_SOUND=1`.
pub fn agent_sound_enabled() -> bool {
    if std::env::var("KUMO_NO_SOUND").is_ok() {
        return false;
    }
    cached_config().agent_sound
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
fn home_dir() -> Option<PathBuf> {
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
        assert!(agent_sound_enabled(), "agent-sound should default to on");
        write(&cfg_dir.join("config"), "agent-sound = false\n");
        assert!(!agent_sound_enabled(), "agent-sound = false must disable alerts");
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
        // accent should remain default (spider-verse accent #ff2a5f)
        assert_eq!(ct.accent, ratatui::style::Color::Rgb(0xff, 0x2a, 0x5f));
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
}
