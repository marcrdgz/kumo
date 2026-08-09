#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Parsed user configuration. Mirrors the flat `key = value` config file;
/// future knobs (theme, leader, keymaps, status bar) will extend this struct.
#[derive(Default)]
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
}

impl Config {
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
        cfg
    }
}

/// Load and merge the configuration. The main `config` file wins over the
/// legacy `~/.kumo` file; keys absent from the main file fall back to legacy.
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
    Config::from_map(&map)
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
    let cfg = load_config();
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
    let cfg = load_config();
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
    load_config().update_check
}

/// Whether agent lifecycle transitions play an audible alert. Disabled by
/// `agent-sound = false` in the config file or by setting `KUMO_NO_SOUND=1`.
pub fn agent_sound_enabled() -> bool {
    if std::env::var("KUMO_NO_SOUND").is_ok() {
        return false;
    }
    load_config().agent_sound
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
        let _guards = (
            EnvGuard::set("XDG_RUNTIME_DIR", "/run/user/1000"),
            EnvGuard::unset("TMPDIR"),
        );
        assert_eq!(runtime_dir(), PathBuf::from("/run/user/1000/kumo"));
    }

    #[test]
    fn runtime_dir_falls_back_to_tmp() {
        let _g = ENV_LOCK.lock().unwrap();
        let _guards = (
            EnvGuard::unset("XDG_RUNTIME_DIR"),
            EnvGuard::set("TMPDIR", "/var/folders/xyz"),
        );
        assert_eq!(runtime_dir(), PathBuf::from("/var/folders/xyz/kumo"));
    }

    #[test]
    fn default_is_opencode() {
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
        let _g = ENV_LOCK.lock().unwrap();
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
}
