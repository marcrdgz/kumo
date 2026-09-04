//! Agent skill distribution: the `kumo-agents.md` prompt that teaches
//! every AI pane to keep worktree checkpoints fresh.
//!
//! The file lives at the workspace root (`kumo-agents.md`) and is embedded
//! into the binary so `make install` and `cargo install` can drop it into
//! the user's config without needing the repo checkout.

/// Embedded skill content — always the crate's `kumo-agents.md` (mirrored from repo root).
pub const SKILL: &str = include_str!("../kumo-agents.md");

/// Discovery stub — the short file at `skills/kumo/SKILL.md`.
pub const STUB: &str = include_str!("../../../skills/kumo/SKILL.md");

/// Registry of installable skills (`npx skills add`).
#[derive(Clone, Copy, Debug)]
pub struct SkillMeta {
    pub name: &'static str,
    pub description: &'static str,
    pub content: &'static str,
    pub stub: &'static str,
}

pub const SKILLS: &[SkillMeta] = &[
    SkillMeta { name: "kumo", description: "Kumo terminal multiplexer — worktrees, checkpoints, orchestration", content: SKILL, stub: STUB },
];

/// Find a skill by name (accepts `kumo` / `kumo-agents`).
pub fn find(name: &str) -> Option<&'static SkillMeta> {
    let n = name.trim().to_ascii_lowercase();
    SKILLS.iter().find(|s| s.name == n || format!("{}-agents", s.name) == n || n == "kumo-agents")
}

/// Where the skill should live for the human (`~/.config/kumo/kumo-agents.md`).
pub fn installed_path() -> std::path::PathBuf {
    crate::config::config_dir().join("kumo-agents.md")
}

fn strip_jsonc_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    let mut in_block = false;
    let mut in_line = false;
    while let Some(c) = chars.next() {
        if in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block = false;
            }
            continue;
        }
        if in_line {
            if c == '\n' {
                in_line = false;
                out.push(c);
            }
            continue;
        }
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c == '/' {
            if let Some(&next) = chars.peek() {
                if next == '/' {
                    chars.next();
                    in_line = true;
                    continue;
                }
                if next == '*' {
                    chars.next();
                    in_block = true;
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// Whether checkpoints are enabled via `[checkpoints] enabled` (default true).
fn checkpoints_enabled() -> bool {
    crate::config::checkpoints_enabled()
}

/// Ensure the skill is installed at `installed_path()` and also mirrored to
/// well-known agent global skill dirs so `opencode` / `claude` / `codex` pick
/// it up without extra steps. For `npx skills add` managers we install the
/// discoverable stub at `skills/kumo/SKILL.md` (agent runs `kumo skills get kumo --full`
/// to fetch the versioned guide). For legacy direct file loads we also mirror the full
/// guide as `kumo-agents.md`.
/// Returns the primary path written (if any) for logging.
/// No-ops when `[checkpoints] enabled = false` in `config.toml`.
pub fn ensure_installed() -> Option<std::path::PathBuf> {
    if !checkpoints_enabled() {
        return None;
    }
    let primary = installed_path();
    let mut written_primary = None;

    // Always ensure the primary Kumo copy exists (idempotent, overwrites only if
    // content changed — avoids churning mtime).
    if let Some(parent) = primary.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let needs_write = std::fs::read_to_string(&primary)
        .map(|existing| existing != SKILL)
        .unwrap_or(true);
    if needs_write {
        if std::fs::write(&primary, SKILL).is_ok() {
            written_primary = Some(primary.clone());
        }
    } else {
        written_primary = Some(primary.clone());
    }

    let home = crate::config::home_dir();
    if let Some(home) = home {
        // Legacy flat mirrors (read by older agent configs that load a single file).
        // Created unconditionally so `kumo skills install --global` always visibly
        // updates opencode/claude/codex even if the user never created the skills
        // subdir. Old behaviour gated on `parent.is_dir()` left `opencode` silently
        // unwired after `make install`.
        let flat_candidates = [
            home.join(".config").join("opencode").join("kumo-agents.md"),
            home.join(".opencode").join("kumo-agents.md"),
            home.join(".claude").join("kumo-agents.md"),
            home.join(".codex").join("kumo-agents.md"),
        ];
        for path in flat_candidates {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let needs = std::fs::read_to_string(&path)
                .map(|e| e != SKILL)
                .unwrap_or(true);
            if needs {
                let _ = std::fs::write(&path, SKILL);
            }
        }

        // `npx skills add` discoverable stub: `skills/kumo/SKILL.md`.
        // `opencode` 1.18+ also scans `~/.config/opencode/skills/` for skills.
        // Always create the directory and write the stub so the skill is
        // discoverable immediately after `make install` / `kumo skills install`.
        let stub_candidates = [
            home.join(".config").join("opencode").join("skills").join("kumo").join("SKILL.md"),
            home.join(".agents").join("skills").join("kumo").join("SKILL.md"),
            home.join(".claude").join("skills").join("kumo").join("SKILL.md"),
            home.join(".codex").join("skills").join("kumo").join("SKILL.md"),
        ];
        for path in stub_candidates {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let needs = std::fs::read_to_string(&path)
                .map(|e| e != STUB)
                .unwrap_or(true);
            if needs {
                let _ = std::fs::write(&path, STUB);
            }
        }

        // `~/.opencode/skills` is a legacy variant some setups use.
        let legacy_skill = home.join(".opencode").join("skills").join("kumo-agents.md");
        if let Some(parent) = legacy_skill.parent() {
            let _ = std::fs::create_dir_all(parent);
            let needs = std::fs::read_to_string(&legacy_skill)
                .map(|e| e != SKILL)
                .unwrap_or(true);
            if needs {
                let _ = std::fs::write(&legacy_skill, SKILL);
            }
        }

        // Ensure opencode's global AGENTS.md always contains checkpoint instructions
        // so the agent uses them automatically without needing to call `skill({name:"kumo"})`.
        // opencode loads `~/.config/opencode/AGENTS.md` on every session (see /docs/rules).
        let global_agents = home.join(".config").join("opencode").join("AGENTS.md");
        if let Some(parent) = global_agents.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Marker to identify our auto-injected block; we update it when SKILL changes.
        let header = "<!-- kumo-checkpoints: auto-installed by `kumo skills install --global` — do not edit manually, re-run install to update -->";
        let block = format!("{header}\n{}", SKILL);
        let needs_agents = std::fs::read_to_string(&global_agents)
            .map(|existing| !existing.contains(header) || !existing.contains(SKILL))
            .unwrap_or(true);
        if needs_agents {
            // If file exists, preserve user content and append/replace our block.
            let mut existing = std::fs::read_to_string(&global_agents).unwrap_or_default();
            if existing.contains(header) {
                // Replace old block between header and next `<!--` or end.
                if let Some(start) = existing.find(header) {
                    let before = &existing[..start];
                    // Our block is header + SKILL; replace from header to end with fresh block.
                    existing = format!("{}\n\n{block}\n", before.trim_end());
                } else {
                    existing = format!("{existing}\n\n{block}\n");
                }
            } else if existing.trim().is_empty() {
                existing = format!("{block}\n");
            } else {
                existing = format!("{existing}\n\n{block}\n");
            }
            let _ = std::fs::write(&global_agents, existing);
        }

        // Also ensure opencode config `instructions` points at kumo-agents.md for projects
        // that rely on instruction files rather than AGENTS.md. We handle both
        // `opencode.json` and `opencode.jsonc` (jsonc may contain comments, so we
        // fall back to string check if JSON parse fails). We do not overwrite
        // existing instructions, we just ensure kumo is present.
        for cfg_name in ["opencode.json", "opencode.jsonc"] {
            let opencode_cfg = home.join(".config").join("opencode").join(cfg_name);
            if opencode_cfg.is_file() {
                if let Ok(content) = std::fs::read_to_string(&opencode_cfg) {
                    if content.contains("kumo-agents.md") {
                        continue;
                    }
                    // Try to parse as JSON and inject instructions if missing.
                    // First try direct JSON, then jsonc (strip // and /* */ comments outside strings).
                    let mut parsed: Option<serde_json::Value> = serde_json::from_str(&content).ok()
                        .or_else(|| serde_json::from_str(&strip_jsonc_comments(&content)).ok());
                    if let Some(mut v) = parsed.take() {
                        if let Some(obj) = v.as_object_mut() {
                            let entry = obj.entry("instructions").or_insert(serde_json::Value::Array(vec![]));
                            if let Some(arr) = entry.as_array_mut() {
                                let candidate = "~/.config/kumo/kumo-agents.md";
                                if !arr.iter().any(|x| x.as_str() == Some(candidate)) {
                                    arr.push(serde_json::Value::String(candidate.to_string()));
                                    if let Ok(out) = serde_json::to_string_pretty(&v) {
                                        let _ = std::fs::write(&opencode_cfg, out);
                                    }
                                }
                            }
                        }
                    }
                }
            } else if cfg_name == "opencode.json" {
                // Create minimal opencode.json with instructions if none exists and its
                // jsonc counterpart also doesn't exist. We only do this if ~/.config/opencode exists.
                let jsonc = home.join(".config").join("opencode").join("opencode.jsonc");
                if !jsonc.is_file() && home.join(".config").join("opencode").is_dir() {
                    let v = serde_json::json!({
                        "$schema": "https://opencode.ai/config.json",
                        "instructions": ["~/.config/kumo/kumo-agents.md"]
                    });
                    if let Ok(out) = serde_json::to_string_pretty(&v) {
                        let _ = std::fs::write(&opencode_cfg, out);
                    }
                }
            }
        }
    }

    written_primary
}

/// Remove the skill from all well-known locations (mirrors `ensure_installed`).
/// Cleans: primary `~/.config/kumo/kumo-agents.md`, flat mirrors, stubs,
/// legacy variant, the auto-injected block in `~/.config/opencode/AGENTS.md`,
/// and the `instructions` entry in `opencode.json[c]`. Does not touch
/// `worktrees.json` (checkpoint history) — pass `prune` to also clear it.
/// Returns the list of paths/files that were actually removed or modified.
pub fn remove_installed() -> Vec<std::path::PathBuf> {
    let mut removed = Vec::new();
    let header = "<!-- kumo-checkpoints: auto-installed by `kumo skills install --global` — do not edit manually, re-run install to update -->";

    // Primary
    let primary = installed_path();
    if primary.is_file() {
        let _ = std::fs::remove_file(&primary);
        removed.push(primary);
    }

    let Some(home) = crate::config::home_dir() else {
        return removed;
    };

    // Flat mirrors
    for path in [
        home.join(".config").join("opencode").join("kumo-agents.md"),
        home.join(".opencode").join("kumo-agents.md"),
        home.join(".claude").join("kumo-agents.md"),
        home.join(".codex").join("kumo-agents.md"),
    ] {
        if path.is_file() {
            // Only remove if it matches our SKILL (avoid deleting user-edited files with other content)
            let is_ours = std::fs::read_to_string(&path).map(|c| c == SKILL).unwrap_or(false);
            if is_ours {
                let _ = std::fs::remove_file(&path);
                removed.push(path);
            } else if path.is_file() {
                // If header present but content diverged, still remove if it contains header
                if let Ok(c) = std::fs::read_to_string(&path) {
                    if c.contains(header) || c.contains("kumo worktree set --comment") {
                        let _ = std::fs::remove_file(&path);
                        removed.push(path);
                    }
                }
            }
        }
    }

    // Stubs
    for path in [
        home.join(".config").join("opencode").join("skills").join("kumo").join("SKILL.md"),
        home.join(".agents").join("skills").join("kumo").join("SKILL.md"),
        home.join(".claude").join("skills").join("kumo").join("SKILL.md"),
        home.join(".codex").join("skills").join("kumo").join("SKILL.md"),
    ] {
        if path.is_file() {
            let is_ours = std::fs::read_to_string(&path).map(|c| c == STUB).unwrap_or(false);
            if is_ours {
                let _ = std::fs::remove_file(&path);
                removed.push(path.clone());
                // Try to remove empty parent `kumo` dir
                if let Some(parent) = path.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
            }
        }
    }

    // Legacy variant
    let legacy_skill = home.join(".opencode").join("skills").join("kumo-agents.md");
    if legacy_skill.is_file() {
        let is_ours = std::fs::read_to_string(&legacy_skill).map(|c| c == SKILL).unwrap_or(false);
        if is_ours {
            let _ = std::fs::remove_file(&legacy_skill);
            removed.push(legacy_skill);
        }
    }

    // Global AGENTS.md block
    let global_agents = home.join(".config").join("opencode").join("AGENTS.md");
    if global_agents.is_file() {
        if let Ok(content) = std::fs::read_to_string(&global_agents) {
            if content.contains(header) {
                if let Some(start) = content.find(header) {
                    let before = content[..start].trim_end().to_string();
                    let new_content = if before.is_empty() {
                        String::new()
                    } else {
                        format!("{before}\n")
                    };
                    if new_content.is_empty() {
                        let _ = std::fs::remove_file(&global_agents);
                    } else {
                        let _ = std::fs::write(&global_agents, &new_content);
                    }
                    removed.push(global_agents.clone());
                }
            }
        }
    }

    // opencode.json[c] instructions
    for cfg_name in ["opencode.json", "opencode.jsonc"] {
        let opencode_cfg = home.join(".config").join("opencode").join(cfg_name);
        if opencode_cfg.is_file() {
            if let Ok(content) = std::fs::read_to_string(&opencode_cfg) {
                if content.contains("kumo-agents.md") {
                    let raw = strip_jsonc_comments(&content);
                    if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if let Some(obj) = v.as_object_mut() {
                            let mut changed = false;
                            if let Some(arr) = obj.get_mut("instructions").and_then(|x| x.as_array_mut()) {
                                let before = arr.len();
                                arr.retain(|x| x.as_str() != Some("~/.config/kumo/kumo-agents.md"));
                                if arr.len() != before {
                                    changed = true;
                                }
                            }
                            if changed {
                                if let Ok(out) = serde_json::to_string_pretty(&v) {
                                    let _ = std::fs::write(&opencode_cfg, out);
                                    removed.push(opencode_cfg.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    removed
}
