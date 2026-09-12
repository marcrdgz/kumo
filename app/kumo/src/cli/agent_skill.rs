//! Installation targets for Kumo's bundled orchestration skill.
//!
//! The control CLI owns the skill content; the TUI settings panel reuses this
//! registry to discover and install the same file in supported agent homes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const KUMO_AGENT_SKILL: &str = include_str!("../../../../skills/kumo/SKILL.md");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentSkillTarget {
    Codex,
    Claude,
    Gemini,
    OpenCode,
}

pub const AGENT_SKILL_TARGETS: [AgentSkillTarget; 4] = [
    AgentSkillTarget::Codex,
    AgentSkillTarget::Claude,
    AgentSkillTarget::Gemini,
    AgentSkillTarget::OpenCode,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentSkillStatus {
    Missing,
    Current,
    Outdated,
}

impl AgentSkillTarget {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Gemini => "Gemini CLI",
            Self::OpenCode => "OpenCode",
        }
    }

    pub fn display_path(self) -> Result<PathBuf> {
        self.path()
    }

    pub fn path(self) -> Result<PathBuf> {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .context("HOME is not set; cannot resolve the agent skill directory")?;
        Ok(match self {
            Self::Codex => home.join(".agents/skills/kumo/SKILL.md"),
            Self::Claude => home.join(".claude/skills/kumo/SKILL.md"),
            Self::Gemini => home.join(".gemini/skills/kumo/SKILL.md"),
            Self::OpenCode => std::env::var_os("XDG_CONFIG_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"))
                .join("opencode/skills/kumo/SKILL.md"),
        })
    }

    pub fn status(self) -> AgentSkillStatus {
        self.path()
            .map(|path| status_at(&path))
            .unwrap_or(AgentSkillStatus::Missing)
    }
}

fn status_at(path: &Path) -> AgentSkillStatus {
    match std::fs::read(path) {
        Ok(content) if content == KUMO_AGENT_SKILL.as_bytes() => AgentSkillStatus::Current,
        Ok(_) => AgentSkillStatus::Outdated,
        Err(_) => AgentSkillStatus::Missing,
    }
}

pub fn install(target: AgentSkillTarget) -> Result<PathBuf> {
    let path = target.path()?;
    install_to(&path)?;
    Ok(path)
}

pub fn uninstall(target: AgentSkillTarget) -> Result<PathBuf> {
    let path = target.path()?;
    uninstall_from(&path)?;
    Ok(path)
}

pub fn install_to(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create skill directory {}", parent.display()))?;
    }
    std::fs::write(path, KUMO_AGENT_SKILL)
        .with_context(|| format!("failed to install Kumo skill at {}", path.display()))
}

fn uninstall_from(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("failed to remove Kumo skill at {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(prefix: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn install_to_creates_parent_and_writes_bundled_skill() {
        let root = TempRoot::new("kumo-agent-skill-install");
        let path = root.path().join("nested/kumo/SKILL.md");

        install_to(&path).unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), KUMO_AGENT_SKILL);
    }

    #[test]
    fn status_at_distinguishes_missing_current_and_outdated_skills() {
        let root = TempRoot::new("kumo-agent-skill-status");
        let missing = root.path().join("missing/SKILL.md");
        let current = root.path().join("current/SKILL.md");
        let outdated = root.path().join("outdated/SKILL.md");

        install_to(&current).unwrap();
        install_to(&outdated).unwrap();
        std::fs::write(&outdated, "older skill").unwrap();

        assert_eq!(status_at(&missing), AgentSkillStatus::Missing);
        assert_eq!(status_at(&current), AgentSkillStatus::Current);
        assert_eq!(status_at(&outdated), AgentSkillStatus::Outdated);
    }

    #[test]
    fn uninstall_removes_only_the_skill_file_and_is_idempotent() {
        let root = TempRoot::new("kumo-agent-skill-uninstall");
        let path = root.path().join("kumo/SKILL.md");
        install_to(&path).unwrap();

        uninstall_from(&path).unwrap();
        assert!(!path.exists());
        assert!(path.parent().unwrap().exists());
        uninstall_from(&path).unwrap();
    }
}
