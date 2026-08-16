//! Per-process CPU/RAM sampling for the sidebar's agent micro-pill metrics.
//!
//! The daemon owns every pane's process tree, so it can report live numbers
//! instead of shipping placeholder text. Instantaneous CPU is the CPU time
//! consumed since the previous sample divided by the wall-clock elapsed, so a
//! busy agent reads ~100% per core while an idle one settles near zero.
//!
//! Sampling runs only for AI panes (usually a handful), at the agent-status
//! refresh cadence, via `ps` on macOS and `/proc` on Linux. Any failure yields
//! a `(0.0, 0)` and the PID's delta state is dropped, so a missing process is
//! never mistaken for a busy one.

use std::collections::HashMap;
use std::time::Instant;

/// CPU-time snapshot of one process, for delta-based instantaneous CPU%.
struct CpuState {
    cpu_secs: f64,
    at: Instant,
}

/// Stateful sampler: keeps the previous CPU-time snapshot per PID so each call
/// returns the CPU consumed *between* calls.
#[derive(Default)]
pub struct ProcSampler {
    prev: HashMap<u32, CpuState>,
}

impl ProcSampler {
    /// Sample `pid`'s instantaneous CPU% (of one core) and resident memory in
    /// KiB. `(0.0, 0)` when the process is gone or the platform cannot read it.
    pub fn sample(&mut self, pid: u32) -> (f32, u64) {
        let (cpu_secs, rss_kb) = match cpu_time_rss(pid) {
            Some(v) => v,
            None => {
                self.prev.remove(&pid);
                return (0.0, 0);
            }
        };
        let now = Instant::now();
        let cpu = match self.prev.get(&pid) {
            Some(prev) if now > prev.at => {
                let wall = now.duration_since(prev.at).as_secs_f64();
                if wall > 0.0 {
                    ((cpu_secs - prev.cpu_secs).max(0.0) / wall * 100.0) as f32
                } else {
                    0.0
                }
            }
            _ => 0.0,
        };
        self.prev.insert(pid, CpuState { cpu_secs, at: now });
        (cpu.clamp(0.0, 999.0), rss_kb)
    }

    /// Drop any delta state for `pid` (pane closed / agent killed), so the
    /// next sample starts fresh instead of reporting a huge "usage" spike.
    pub fn forget(&mut self, pid: u32) {
        self.prev.remove(&pid);
    }
}

#[cfg(target_os = "macos")]
fn cpu_time_rss(pid: u32) -> Option<(f64, u64)> {
    use std::process::Command;
    let out = Command::new("ps").args(["-p", &pid.to_string(), "-o", "time=,rss="]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8(out.stdout).ok()?;
    let mut fields = line.split_whitespace();
    let time = fields.next()?;
    let rss: u64 = fields.next()?.parse().ok()?;
    Some((mmss_to_secs(time)?, rss))
}

#[cfg(target_os = "linux")]
fn cpu_time_rss(pid: u32) -> Option<(f64, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The `comm` field can contain spaces/parens; fields restart after the
    // last ')'. After it, index 0 is field 3 (state); utime is field 14 and
    // stime field 15, so utime = index 11, stime = index 12.
    let close = stat.rfind(')')?;
    let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    let utime: f64 = rest.get(11)?.parse().ok()?;
    let stime: f64 = rest.get(12)?.parse().ok()?;
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    if ticks <= 0.0 {
        return None;
    }
    let cpu_secs = (utime + stime) / ticks;
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let rss_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    Some((cpu_secs, rss_pages * page / 1024))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn cpu_time_rss(_pid: u32) -> Option<(f64, u64)> {
    None
}

/// `ps`'s `time=` cumulative CPU format: `MM:SS` (minutes may exceed 59) or
/// `HH:MM:SS` past an hour.
fn mmss_to_secs(s: &str) -> Option<f64> {
    let parts: Vec<u64> = s.split(':').map(|p| p.parse().ok()).collect::<Option<Vec<_>>>()?;
    match parts.as_slice() {
        [mm, ss] => Some(*mm as f64 * 60.0 + *ss as f64),
        [hh, mm, ss] => Some(*hh as f64 * 3600.0 + *mm as f64 * 60.0 + *ss as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ps_time_format() {
        assert_eq!(mmss_to_secs("00:00"), Some(0.0));
        assert_eq!(mmss_to_secs("12:34"), Some(754.0));
        assert_eq!(mmss_to_secs("90:00"), Some(5400.0));
        assert_eq!(mmss_to_secs("1:02:03"), Some(3723.0));
        assert_eq!(mmss_to_secs("bogus"), None);
    }
}
