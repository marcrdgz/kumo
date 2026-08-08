use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Resolve the AI CLI command line to run inside the AI pane.
///
/// Precedence: `KUMO_AI_CMD` env var, then a `~/.kumo` line config,
/// then a built-in default.
pub fn ai_command() -> (String, Vec<String>) {    if let Ok(raw) = std::env::var("KUMO_AI_CMD") {
        if !raw.trim().is_empty() {
            return split_cmd(&raw);
        }
    }
    if let Some(home) = std::env::var("HOME").ok() {
        if let Ok(content) = std::fs::read_to_string(PathBuf::from(&home).join(".kumo")) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut parts = line.splitn(2, '=');
                if parts.next() == Some("ai_cmd") {
                    if let Some(v) = parts.next() {
                        let v = v.trim().trim_matches('"').trim_matches('\'');
                        if !v.is_empty() {
                            return split_cmd(v);
                        }
                    }
                }
            }
        }
    }
    (String::from("opencode"), Vec::new())
}

/// Working directory for the AI pane. Prefers the persisted workspace
/// (`~/.kumo/workspace.json`) so opencode runs inside the project and
/// `@file` references resolve; falls back to `$HOME`.
pub fn ai_cwd() -> PathBuf {
    if let Some(home) = std::env::var("HOME").ok() {
        let wp = PathBuf::from(&home).join(".kumo").join("workspace.json");
        if let Ok(ws) = std::fs::read_to_string(&wp) {
            let ws = ws.trim();
            if !ws.is_empty() {
                let pb = PathBuf::from(ws);
                if pb.is_dir() {
                    return pb;
                }
            }
        }
    }
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// Split a command line string into program + args (space separated).
fn split_cmd(raw: &str) -> (String, Vec<String>) {
    let mut it = raw.split_whitespace();
    let program = it.next().unwrap_or("opencode").to_string();
    let args: Vec<String> = it.map(|s| s.to_string()).collect();
    (program, args)
}

/// The user's login shell, falling back to zsh.
pub fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

static RESOLVE_CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

fn resolve_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    RESOLVE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_is_opencode() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("KUMO_AI_CMD");
        let (prog, args) = ai_command();
        assert_eq!(prog, "opencode");
        assert!(args.is_empty());
    }

    #[test]
    fn env_var_overrides() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("KUMO_AI_CMD", "claude --model sonnet");
        let (prog, args) = ai_command();
        assert_eq!(prog, "claude");
        assert_eq!(args, vec!["--model", "sonnet"]);
        std::env::remove_var("KUMO_AI_CMD");
    }

    #[test]
    fn split_cmd_handles_plain_program() {
        let (prog, args) = split_cmd("opencode");
        assert_eq!(prog, "opencode");
        assert!(args.is_empty());
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

