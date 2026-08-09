//! Self-update support.
//!
//! Version detection is done through the `gh` CLI (GitHub CLI), which the
//! release pipeline publishes against. The stable channel installs through
//! cargo-dist's generated `installer.sh` / `installer.ps1` (which verify the
//! artifact checksum and handle the platform-specific install path), while the
//! nightly channel downloads the archive directly and swaps the binary.
//!
//! A TTL'd cache under the state dir backs the startup notification so kumo
//! never hits the GitHub API on every launch.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;

pub const REPO: &str = "marcrdgz/kumo";
pub const APP: &str = "kumo";
const STABLE_TTL: u64 = 24 * 3600;
const NIGHTLY_TTL: u64 = 6 * 3600;

/// Release channel a check targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Nightly,
}

impl Channel {
    pub fn label(&self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nightly => "nightly",
        }
    }
}

/// A remote release, as resolved from GitHub.
#[derive(Clone, Debug)]
pub struct Latest {
    pub tag: String,
    pub version: Option<semver::Version>,
    pub created_at: Option<String>,
}

/// What `kumo update` concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    Available,
    Updated,
}

/// Options parsed from `kumo update [--nightly] [--check] [--tag <tag>]`.
#[derive(Default, Debug)]
pub struct UpdateOpts {
    pub nightly: bool,
    pub check: bool,
    /// Hidden: force-install a specific release tag (dev tool to hop between tags).
    pub tag: Option<String>,
}

/// Notice shown at startup when a newer release is available.
#[derive(Clone, Debug)]
pub struct UpdateNotice {
    /// Stable: the version. Nightly: the release `created_at`. Persisted so a
    /// dismissed notice does not reappear for the same release.
    pub key: String,
    /// Human-readable version to render in the banner.
    pub display: String,
}

/// Persisted check state.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct UpdateCache {
    pub last_check: Option<u64>,
    pub channel: Option<String>,
    pub latest_tag: Option<String>,
    pub latest_version: Option<String>,
    pub nightly_at: Option<String>,
    pub dismissed_key: Option<String>,
    pub installed_nightly_at: Option<String>,
}

// ----- cache persistence -----

fn cache_path() -> PathBuf {
    config::state_dir().join("update-check.json")
}

pub fn read_cache() -> UpdateCache {
    std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_cache(cache: &UpdateCache) {
    if let Some(parent) = cache_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(s) = serde_json::to_string_pretty(cache) {
        let _ = std::fs::write(cache_path(), s);
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ----- gh helpers -----

pub fn gh_available() -> bool {
    Command::new("gh")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn gh(args: &[&str]) -> Result<String> {
    let out = Command::new("gh")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("failed to spawn `gh` (install the GitHub CLI and run `gh auth login`)")?;
    if !out.status.success() {
        bail!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ----- version helpers -----

fn parse_version_from_tag(tag: &str) -> Option<semver::Version> {
    let tag = tag.strip_prefix("kumo-v").or_else(|| tag.strip_prefix('v')).unwrap_or(tag);
    semver::Version::parse(tag).ok()
}

/// The installed version, if this build is a real release (not `0.0.0-dev`).
fn current_version() -> Option<semver::Version> {
    let v = semver::Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
    if v.major == 0 && v.minor == 0 && v.patch == 0 {
        return None;
    }
    if v.pre.as_str().contains("dev") {
        return None;
    }
    Some(v)
}

/// Display channel for `kumo --version`.
pub fn current_channel_label() -> &'static str {
    let v = env!("CARGO_PKG_VERSION");
    if cfg!(debug_assertions) || v.contains("dev") {
        "dev"
    } else if v.contains("nightly") {
        "nightly"
    } else {
        "stable"
    }
}

fn target_triple() -> String {
    let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", _) => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("windows", _) => "x86_64-pc-windows-msvc",
        _ => "x86_64-unknown-linux-gnu",
    };
    triple.to_string()
}

// ----- release resolution -----

pub fn resolve_latest(nightly: bool) -> Result<Latest> {
    if nightly {
        let out = gh(&[
            "release", "view", "nightly", "--repo", REPO, "--json", "tagName,createdAt", "--jq",
            ".tagName + \" \" + .createdAt",
        ])?;
        let mut it = out.splitn(2, ' ');
        let tag = it.next().unwrap_or("nightly").to_string();
        let created_at = it.next().map(|s| s.to_string());
        Ok(Latest { tag: tag.clone(), version: parse_version_from_tag(&tag), created_at })
    } else {
        let tag = gh(&["release", "view", "--repo", REPO, "--json", "tagName", "--jq", ".tagName"])?;
        Ok(Latest { tag: tag.clone(), version: parse_version_from_tag(&tag), created_at: None })
    }
}

fn resolve_tag(tag: &str) -> Result<Latest> {
    let out = gh(&[
        "release", "view", tag, "--repo", REPO, "--json", "tagName,createdAt", "--jq",
        ".tagName + \" \" + .createdAt",
    ])?;
    let mut it = out.splitn(2, ' ');
    let resolved = it.next().unwrap_or(tag).to_string();
    Ok(Latest {
        tag: resolved.clone(),
        version: parse_version_from_tag(&resolved),
        created_at: it.next().map(|s| s.to_string()),
    })
}

fn is_update_needed(channel: Channel, latest: &Latest, force: bool) -> bool {
    if force {
        return true;
    }
    match channel {
        Channel::Stable => match current_version() {
            None => false,
            Some(cur) => latest.version.as_ref().is_some_and(|v| v > &cur),
        },
        Channel::Nightly => match (&latest.created_at, &read_cache().installed_nightly_at) {
            (Some(now), Some(last)) => now != last,
            (Some(_), None) => true,
            _ => false,
        },
    }
}

// ----- install -----

fn temp_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("kumo-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).context("failed to create temp dir")?;
    Ok(dir)
}

fn remove_dir(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

fn install_stable(latest: &Latest) -> Result<()> {
    let assets =
        gh(&["release", "view", &latest.tag, "--repo", REPO, "--json", "assets", "--jq", ".assets[].name"])?;
    let installer = assets
        .lines()
        .find(|n| n.ends_with("-installer.sh") || n.ends_with("-installer.ps1"))
        .context("no cargo-dist installer found on the release")?;
    let dir = temp_dir()?;
    gh(&[
        "release", "download", &latest.tag, "--repo", REPO, "--pattern", installer, "--dir",
        dir.to_str().unwrap(),
    ])?;
    let script = dir.join(installer);
    let status = if cfg!(windows) {
        Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .status()
    } else {
        Command::new("sh").arg(&script).status()
    };
    let ok = status.context("failed to run the kumo installer")?.success();
    remove_dir(&dir);
    if !ok {
        bail!("the kumo installer reported a failure");
    }
    Ok(())
}

fn install_nightly(latest: &Latest) -> Result<()> {
    let dir = temp_dir()?;
    let pattern = format!("{APP}-{}.tar.xz", target_triple());
    gh(&[
        "release", "download", &latest.tag, "--repo", REPO, "--pattern", &pattern, "--dir",
        dir.to_str().unwrap(),
    ])?;
    let archive = dir.join(&pattern);
    if !archive.is_file() {
        bail!("expected archive {} not found", archive.display());
    }
    let extract = dir.join("extract");
    std::fs::create_dir_all(&extract)?;
    let ok = Command::new("tar")
        .args(["-xf"])
        .arg(&archive)
        .arg("-C")
        .arg(&extract)
        .status()
        .context("`tar` is required to install a nightly build")?
        .success();
    if !ok {
        remove_dir(&dir);
        bail!("failed to extract {}", archive.display());
    }
    let binary = find_binary(&extract)?;
    swap_binary(&binary)?;
    remove_dir(&dir);
    Ok(())
}

fn find_binary(dir: &Path) -> Result<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().map(|n| n == APP).unwrap_or(false) {
                return Ok(p);
            }
        }
    }
    bail!("binary `{APP}` not found in the archive")
}

fn swap_binary(new_bin: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("cannot determine the current executable path")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let dir = exe.parent().context("current executable has no parent directory")?;
    let staged = dir.join(format!(".{APP}.update.new"));
    std::fs::copy(new_bin, &staged).context("failed to stage the new binary")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&staged, &exe).context("failed to replace the kumo binary")?;
    Ok(())
}

// ----- CLI entry -----

pub fn parse_args(args: &[String]) -> Result<UpdateOpts> {
    let mut opts = UpdateOpts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--nightly" | "-n" => opts.nightly = true,
            "--check" | "-c" => opts.check = true,
            "--tag" => {
                i += 1;
                opts.tag = Some(args.get(i).context("--tag requires a value")?.clone());
            }
            other => bail!("unknown option: {other}"),
        }
        i += 1;
    }
    Ok(opts)
}

pub fn update(opts: &UpdateOpts) -> Result<Outcome> {
    if !gh_available() {
        bail!("`gh` is required to update kumo (install the GitHub CLI and run `gh auth login`)");
    }
    let channel = if opts.nightly { Channel::Nightly } else { Channel::Stable };
    let latest = match &opts.tag {
        Some(tag) => resolve_tag(tag)?,
        None => resolve_latest(opts.nightly)?,
    };

    if !is_update_needed(channel, &latest, opts.tag.is_some()) {
        match channel {
            Channel::Stable if current_version().is_none() => {
                println!(
                    "kumo {} is up to date (dev build; use --tag to install a specific release)",
                    env!("CARGO_PKG_VERSION")
                );
            }
            _ => println!("kumo {} is up to date", env!("CARGO_PKG_VERSION")),
        }
        return Ok(Outcome::UpToDate);
    }

    if opts.check {
        println!("update available: {}", latest.tag);
        return Ok(Outcome::Available);
    }

    match channel {
        Channel::Stable => install_stable(&latest)?,
        Channel::Nightly => install_nightly(&latest)?,
    }

    let key = dismiss_key(channel, &latest);
    let mut cache = read_cache();
    cache.channel = Some(channel.label().to_string());
    cache.dismissed_key = Some(key);
    if opts.nightly {
        cache.installed_nightly_at = latest.created_at.clone();
    }
    write_cache(&cache);

    println!("kumo updated to {}", latest.tag);
    println!("restart kumo to use the new version");
    Ok(Outcome::Updated)
}

// ----- startup notification -----

fn dismiss_key(channel: Channel, latest: &Latest) -> String {
    match channel {
        Channel::Stable => latest
            .version
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_else(|| latest.tag.clone()),
        Channel::Nightly => latest.created_at.clone().unwrap_or_else(|| latest.tag.clone()),
    }
}

/// Best-effort check with a TTL cache. Never blocks or fails hard: callers must
/// tolerate `None` (no gh, network down, or nothing cached yet).
fn check_latest_cached(nightly: bool) -> Option<Latest> {
    let mut cache = read_cache();
    let ttl = if nightly { NIGHTLY_TTL } else { STABLE_TTL };
    let now = unix_now();
    let stale = cache.last_check.is_none_or(|t| now.saturating_sub(t) >= ttl);
    if !stale {
        let tag = cache.latest_tag.clone()?;
        let version = cache.latest_version.as_deref().and_then(|v| semver::Version::parse(v).ok());
        return Some(Latest { tag, version, created_at: cache.nightly_at.clone() });
    }
    let latest = resolve_latest(nightly).ok()?;
    cache.last_check = Some(now);
    cache.channel = Some(if nightly { "nightly" } else { "stable" }.to_string());
    cache.latest_tag = Some(latest.tag.clone());
    cache.latest_version = latest.version.as_ref().map(|v| v.to_string());
    cache.nightly_at = latest.created_at.clone();
    write_cache(&cache);
    Some(latest)
}

/// Called from a background thread at startup. Returns a notice when a newer
/// release is available and the user has not already dismissed it.
pub fn poll_update_notice() -> Option<UpdateNotice> {
    if !config::update_check_enabled() {
        return None;
    }
    let nightly = read_cache().channel.as_deref() == Some("nightly");
    let channel = if nightly { Channel::Nightly } else { Channel::Stable };
    let latest = check_latest_cached(nightly)?;
    let key = dismiss_key(channel, &latest);
    if read_cache().dismissed_key.as_deref() == Some(key.as_str()) {
        return None;
    }
    match channel {
        Channel::Stable => {
            let cur = current_version()?;
            let v = latest.version.as_ref()?;
            if v <= &cur {
                return None;
            }
            Some(UpdateNotice { key, display: v.to_string() })
        }
        Channel::Nightly => {
            let at = latest.created_at.clone()?;
            if Some(at.as_str()) == read_cache().installed_nightly_at.as_deref() {
                return None;
            }
            Some(UpdateNotice { key, display: format!("nightly ({})", short_date(&at)) })
        }
    }
}

/// Persist that the notice for `key` was dismissed.
pub fn dismiss(key: &str) {
    let mut cache = read_cache();
    cache.dismissed_key = Some(key.to_string());
    write_cache(&cache);
}

fn short_date(at: &str) -> String {
    at.split('T').next().unwrap_or(at).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_prefixed_tags() {
        assert_eq!(parse_version_from_tag("v1.0.0-rc.1").unwrap(), semver::Version::parse("1.0.0-rc.1").unwrap());
        assert_eq!(parse_version_from_tag("kumo-v0.2.0").unwrap(), semver::Version::parse("0.2.0").unwrap());
        assert_eq!(parse_version_from_tag("1.0.0").unwrap(), semver::Version::parse("1.0.0").unwrap());
    }

    #[test]
    fn non_version_tags_do_not_parse() {
        assert!(parse_version_from_tag("nightly").is_none());
        assert!(parse_version_from_tag("foo").is_none());
    }

    #[test]
    fn cache_round_trips() {
        let mut cache = UpdateCache::default();
        cache.channel = Some("stable".to_string());
        cache.latest_tag = Some("v1.0.0".to_string());
        cache.latest_version = Some("1.0.0".to_string());
        let json = serde_json::to_string(&cache).unwrap();
        let back: UpdateCache = serde_json::from_str(&json).unwrap();
        assert_eq!(back.channel.as_deref(), Some("stable"));
        assert_eq!(back.latest_tag.as_deref(), Some("v1.0.0"));
    }

    #[test]
    fn stable_dismiss_key_uses_version() {
        let latest = Latest {
            tag: "v1.0.0".to_string(),
            version: Some(semver::Version::parse("1.0.0").unwrap()),
            created_at: None,
        };
        assert_eq!(dismiss_key(Channel::Stable, &latest), "1.0.0");
    }

    #[test]
    fn nightly_dismiss_key_uses_created_at() {
        let latest = Latest {
            tag: "nightly".to_string(),
            version: None,
            created_at: Some("2026-08-09T04:00:00Z".to_string()),
        };
        assert_eq!(dismiss_key(Channel::Nightly, &latest), "2026-08-09T04:00:00Z");
    }
}
