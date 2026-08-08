use std::path::PathBuf;

/// Resolve the AI CLI command line to run inside the AI pane.
///
/// Precedence: `NEOMUX_AI_CMD` env var, then a `~/.neomux` line config,
/// then a built-in default.
pub fn ai_command() -> (String, Vec<String>) {
    if let Ok(raw) = std::env::var("NEOMUX_AI_CMD") {
        if !raw.trim().is_empty() {
            return split_cmd(&raw);
        }
    }
    if let Some(home) = std::env::var("HOME").ok() {
        if let Ok(content) = std::fs::read_to_string(PathBuf::from(&home).join(".neomux")) {
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
/// (`~/.neomux/workspace.json`) so opencode runs inside the project and
/// `@file` references resolve; falls back to `$HOME`.
pub fn ai_cwd() -> PathBuf {
    if let Some(home) = std::env::var("HOME").ok() {
        let wp = PathBuf::from(&home).join(".neomux").join("workspace.json");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_is_opencode() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("NEOMUX_AI_CMD");
        let (prog, args) = ai_command();
        assert_eq!(prog, "opencode");
        assert!(args.is_empty());
    }

    #[test]
    fn env_var_overrides() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("NEOMUX_AI_CMD", "claude --model sonnet");
        let (prog, args) = ai_command();
        assert_eq!(prog, "claude");
        assert_eq!(args, vec!["--model", "sonnet"]);
        std::env::remove_var("NEOMUX_AI_CMD");
    }

    #[test]
    fn split_cmd_handles_plain_program() {
        let (prog, args) = split_cmd("opencode");
        assert_eq!(prog, "opencode");
        assert!(args.is_empty());
    }
}

