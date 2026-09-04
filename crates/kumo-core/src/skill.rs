//! Agent skill distribution: the `kumo-agents.md` prompt that teaches
//! every AI pane to keep worktree checkpoints fresh.
//!
//! The file lives at the workspace root (`kumo-agents.md`) and is embedded
//! into the binary so `make install` and `cargo install` can drop it into
//! the user's config without needing the repo checkout.

/// Embedded skill content — always the crate's `kumo-agents.md` (mirrored from repo root).
pub const SKILL: &str = include_str!("../kumo-agents.md");

/// Orca-style stub — the short discovery file at `skills/kumo/SKILL.md`.
pub const STUB: &str = include_str!("../../../skills/kumo/SKILL.md");

/// Registry of installable skills (Orca-style `npx skills add`).
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

/// Ensure the skill is installed at `installed_path()` and, when those
/// directories exist, also mirrored to well-known agent global skill dirs
/// so `opencode` / `claude` / `codex` pick it up without extra steps.
/// Returns the primary path written (if any) for logging.
pub fn ensure_installed() -> Option<std::path::PathBuf> {
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

    // Opportunistically mirror to agent-specific global skill dirs if they exist.
    // We never create those dirs from scratch — we only write if the user already
    // uses that agent (dir exists), to avoid cluttering home.
    let home = crate::config::home_dir();
    if let Some(home) = home {
        let candidates = [
            home.join(".config").join("opencode").join("kumo-agents.md"),
            home.join(".config").join("opencode").join("skills").join("kumo-agents.md"),
            home.join(".opencode").join("kumo-agents.md"),
            home.join(".claude").join("kumo-agents.md"),
            home.join(".codex").join("kumo-agents.md"),
        ];
        for path in candidates {
            if let Some(parent) = path.parent() {
                if parent.is_dir() {
                    let needs = std::fs::read_to_string(&path)
                        .map(|e| e != SKILL)
                        .unwrap_or(true);
                    if needs {
                        let _ = std::fs::write(&path, SKILL);
                    }
                }
            }
        }
    }

    written_primary
}
