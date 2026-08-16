//! Update management for the desktop app — the "smart" interface that detects
//! and installs updates for both the `kumo` CLI (daemon + TUI, installed via
//! the cargo-dist `install.sh` / nightly archive) and the desktop app itself
//! (the macOS `.dmg`).
//!
//! The TUI keeps its own `kumo update`; this module powers the app's startup
//! bootstrap (auto-install the CLI when missing so the daemon can run) and its
//! in-app update menu. `check_all` never blocks or fails hard — callers must
//! tolerate an empty/`Default` status when the network or the GitHub API is
//! unavailable.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use semver::Version;

use crate::update;

pub const APP: &str = "kumo";
pub const DESKTOP_APP_NAME: &str = "Kumo";
/// Whether one component needs an update (or is not installed at all).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ComponentStatus {
    /// The component is not installed.
    Missing,
    /// Installed and current.
    UpToDate { version: String },
    /// Installed but older than the latest release.
    OutOfDate { current: String, latest: String },
    /// Dev build: updates are not tracked.
    #[default]
    Dev,
}

/// Result of checking both components against the latest release.
#[derive(Clone, Debug, Default)]
pub struct UpdateStatus {
    pub cli: ComponentStatus,
    pub desktop: ComponentStatus,
}

impl UpdateStatus {
    /// Whether anything needs the user's attention (an update is available, or
    /// the CLI is missing).
    pub fn any_update(&self) -> bool {
        matches!(self.cli, ComponentStatus::OutOfDate { .. } | ComponentStatus::Missing)
            || matches!(self.desktop, ComponentStatus::OutOfDate { .. })
    }
}

// ----- release resolution -----

fn channel() -> update::Channel {
    if update::read_cache().channel.as_deref() == Some("nightly") {
        update::Channel::Nightly
    } else {
        update::Channel::Stable
    }
}

fn latest() -> Result<update::Latest> {
    update::resolve_latest(channel() == update::Channel::Nightly)
}

// ----- the installed kumo CLI -----

/// The installed `kumo` binary: `$CARGO_HOME/bin/kumo` first (where the
/// install.sh installer and this bootstrap put it), then `kumo` on `PATH`.
pub fn find_kumo() -> Option<PathBuf> {
    let cargo = update::cargo_home_bin().join(APP);
    if cargo.is_file() {
        return Some(cargo);
    }
    which(APP)
}

/// The installed CLI's version, from `kumo --version`. `None` for dev builds
/// (which print `(dev)` / `0.0.0` and cannot be compared).
pub fn installed_version() -> Option<Version> {
    let path = find_kumo()?;
    let out = std::process::Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    if s.contains("(dev)") || s.contains("0.0.0") {
        return None;
    }
    let token = s.split_whitespace().nth(1)?;
    Version::parse(token).ok()
}

// ----- the installed desktop app -----

/// The app bundle's binary modification time (≈ when it was installed or
/// updated), as unix seconds. Used to judge nightly freshness without extra
/// stamps: a fresh dmg copy has mtime ≥ the release it came from.
fn app_installed_at() -> Option<u64> {
    let exe = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(exe).ok()?;
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs()))
}

// ----- status -----

fn cli_status(latest: &update::Latest) -> ComponentStatus {
    if find_kumo().is_none() {
        return ComponentStatus::Missing;
    }
    match channel() {
        update::Channel::Stable => {
            let Some(cur) = installed_version() else {
                return ComponentStatus::Dev;
            };
            match latest.version.as_ref() {
                Some(v) if v > &cur => ComponentStatus::OutOfDate {
                    current: cur.to_string(),
                    latest: v.to_string(),
                },
                _ => ComponentStatus::UpToDate { version: cur.to_string() },
            }
        }
        update::Channel::Nightly => {
            let now = latest.created_at.as_deref().unwrap_or("");
            let cache = update::read_cache();
            let installed = cache.installed_nightly_at.as_deref().unwrap_or("");
            if !now.is_empty() && now != installed {
                ComponentStatus::OutOfDate {
                    current: "nightly".into(),
                    latest: format!("nightly ({})", short_date(now)),
                }
            } else {
                ComponentStatus::UpToDate { version: "nightly".into() }
            }
        }
    }
}

fn desktop_status(latest: &update::Latest) -> ComponentStatus {
    if !update::is_release_build() {
        return ComponentStatus::Dev;
    }
    match channel() {
        update::Channel::Stable => {
            let cur = env!("CARGO_PKG_VERSION");
            let cur_v = Version::parse(cur).unwrap_or(Version::new(0, 0, 0));
            match latest.version.as_ref() {
                Some(v) if v > &cur_v => ComponentStatus::OutOfDate {
                    current: cur.to_string(),
                    latest: v.to_string(),
                },
                _ => ComponentStatus::UpToDate { version: cur.to_string() },
            }
        }
        update::Channel::Nightly => {
            let installed = app_installed_at().unwrap_or(0);
            let release_at = latest
                .created_at
                .as_deref()
                .and_then(rfc3339_to_epoch)
                .unwrap_or(0);
            if release_at > installed {
                ComponentStatus::OutOfDate {
                    current: "nightly".into(),
                    latest: format!("nightly ({})", short_date(latest.created_at.as_deref().unwrap_or(""))),
                }
            } else {
                ComponentStatus::UpToDate { version: "nightly".into() }
            }
        }
    }
}

/// Best-effort status of the CLI and the desktop app. Never fails: an empty
/// status means the check could not run (offline / API unavailable).
pub fn check_all() -> UpdateStatus {
    let latest = match latest() {
        Ok(l) => l,
        Err(_) => return UpdateStatus::default(),
    };
    UpdateStatus { cli: cli_status(&latest), desktop: desktop_status(&latest) }
}

// ----- install -----

fn install_cli_latest(latest: &update::Latest) -> Result<()> {
    match channel() {
        update::Channel::Stable => update::install_stable(latest)?,
        update::Channel::Nightly => {
            update::install_nightly_to(&update::cargo_home_bin().join(APP), latest)?
        }
    }
    if channel() == update::Channel::Nightly {
        let mut cache = update::read_cache();
        cache.installed_nightly_at = latest.created_at.clone();
        update::write_cache(&cache);
    }
    Ok(())
}

/// Startup bootstrap: install the CLI only when it is missing (the daemon
/// cannot run without it). Returns whether an install happened. Outdated but
/// present binaries are left to the in-app "Update" button.
pub fn install_cli_if_missing() -> Result<bool> {
    if find_kumo().is_some() {
        return Ok(false);
    }
    let latest = latest()?;
    install_cli_latest(&latest)?;
    Ok(true)
}

/// In-app "Update kumo CLI": install when missing or outdated.
pub fn update_cli() -> Result<()> {
    let latest = latest()?;
    if !matches!(cli_status(&latest), ComponentStatus::OutOfDate { .. } | ComponentStatus::Missing) {
        return Ok(());
    }
    install_cli_latest(&latest)
}

/// In-app "Update Kumo Desktop": download the latest `Kumo-<arch>.dmg`,
/// replace `/Applications/Kumo.app` and relaunch it (detached, after the old
/// process exits). The caller is expected to quit right after.
#[cfg(target_os = "macos")]
pub fn update_desktop() -> Result<()> {
    use std::path::Path;

    let latest = latest()?;
    let assets = update::release_assets(&latest.tag)?;
    let dmg = format!("{DESKTOP_APP_NAME}-{}.dmg", dmg_arch());
    let url = assets
        .iter()
        .find(|(name, _)| name == &dmg)
        .map(|(_, url)| url)
        .with_context(|| format!("expected asset {dmg} not found on the release"))?;

    let dir = update::temp_dir()?;
    let dmg_path = dir.join(&dmg);
    update::download(url, &dmg_path)?;

    // Mount, swap the app bundle into /Applications, unmount.
    let mount = dir.join("mount");
    std::fs::create_dir_all(&mount)?;
    let attach = std::process::Command::new("hdiutil")
        .args(["attach", dmg_path.to_str().unwrap_or_default(), "-nobrowse", "-mountpoint", mount.to_str().unwrap_or_default()])
        .status()
        .context("`hdiutil` is required to install the desktop app")?;
    if !attach.success() {
        bail!("failed to mount the kumo desktop disk image");
    }
    let app = mount.join(format!("{DESKTOP_APP_NAME}.app"));
    let applications = Path::new("/Applications");
    let result = (|| -> Result<()> {
        if !app.is_dir() {
            bail!("no Kumo.app inside the disk image");
        }
        let dest = applications.join(format!("{DESKTOP_APP_NAME}.app"));
        let _ = std::fs::remove_dir_all(&dest);
        let cp = std::process::Command::new("cp")
            .args(["-R", app.to_str().unwrap_or_default(), applications.to_str().unwrap_or_default()])
            .status()
            .context("failed to copy Kumo.app into /Applications")?;
        if !cp.success() {
            bail!("failed to install Kumo.app (permissions?)");
        }
        Ok(())
    })();
    let _ = std::process::Command::new("hdiutil").args(["detach", mount.to_str().unwrap_or_default()]).status();
    let _ = std::fs::remove_dir_all(&dir);
    result?;

    // Relaunch the freshly installed bundle once the old process exits.
    let _ = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 1; open -n /Applications/Kumo.app"])
        .spawn();
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn update_desktop() -> Result<()> {
    bail!("desktop self-update is macOS-only")
}

fn dmg_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "x86_64",
    }
}

// ----- helpers -----

/// Find `name` on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn short_date(at: &str) -> String {
    at.split('T').next().unwrap_or(at).to_string()
}

/// Parse a GitHub RFC3339 `created_at` (`YYYY-MM-DDTHH:MM:SSZ`) to unix
/// seconds. Only the UTC `Z`/`+00:00` forms are handled (what the API emits).
fn rfc3339_to_epoch(s: &str) -> Option<u64> {
    let t = s.strip_suffix('Z').or_else(|| s.strip_suffix("+00:00"))?;
    let (date, time) = t.split_once('T')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let mut tm = time.split(':');
    let hour: i64 = tm.next()?.parse().ok()?;
    let minute: i64 = tm.next()?.parse().ok()?;
    let sec: i64 = tm.next()?.split('.').next()?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + sec) as u64)
}

/// Days since the 1970-01-01 epoch for a proleptic-Gregorian date
/// (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_utc() {
        assert_eq!(rfc3339_to_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_epoch("2026-08-16T04:00:00Z"), Some(1786852800));
        assert_eq!(rfc3339_to_epoch("2026-08-16T04:00:00+00:00"), Some(1786852800));
    }

    #[test]
    fn rejects_bad_rfc3339() {
        assert_eq!(rfc3339_to_epoch("not-a-date"), None);
        assert_eq!(rfc3339_to_epoch(""), None);
    }

    #[test]
    fn arm_maps_to_dmg_arch() {
        assert_eq!(dmg_arch(), if std::env::consts::ARCH == "aarch64" { "arm64" } else { "x86_64" });
    }
}
