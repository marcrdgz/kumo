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
fn cpu_time_rss(_pid: u32) -> Option<(f64, u64)> {
    macos_proc_pidinfo_cpu_rss(_pid)
}

#[cfg(target_os = "macos")]
fn macos_proc_pidinfo_cpu_rss(pid: u32) -> Option<(f64, u64)> {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let ret = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if ret != size {
        return None;
    }
    let cpu_secs = info.pti_total_user as f64 / 1_000_000_000.0
        + info.pti_total_system as f64 / 1_000_000_000.0;
    let rss_kb = info.pti_resident_size / 1024;
    Some((cpu_secs, rss_kb))
}

#[cfg(target_os = "linux")]
fn cpu_time_rss(pid: u32) -> Option<(f64, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    let utime: f64 = rest.get(11)?.parse().ok()?;
    let stime: f64 = rest.get(12)?.parse().ok()?;
    static CLK_TCK: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    let ticks = *CLK_TCK.get_or_init(|| unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64);
    if ticks <= 0.0 {
        return None;
    }
    let cpu_secs = (utime + stime) / ticks;
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let rss_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    static PAGESIZE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let page = *PAGESIZE.get_or_init(|| unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64);
    Some((cpu_secs, rss_pages * page / 1024))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn cpu_time_rss(_pid: u32) -> Option<(f64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_ps_time_format() {
        let mmss_to_secs = |s: &str| -> Option<f64> {
            let parts: Vec<u64> = s.split(':').map(|p| p.parse().ok()).collect::<Option<Vec<_>>>()?;
            match parts.as_slice() {
                [mm, ss] => Some(*mm as f64 * 60.0 + *ss as f64),
                [hh, mm, ss] => Some(*hh as f64 * 3600.0 + *mm as f64 * 60.0 + *ss as f64),
                _ => None,
            }
        };
        assert_eq!(mmss_to_secs("00:00"), Some(0.0));
        assert_eq!(mmss_to_secs("12:34"), Some(754.0));
        assert_eq!(mmss_to_secs("90:00"), Some(5400.0));
        assert_eq!(mmss_to_secs("1:02:03"), Some(3723.0));
        assert_eq!(mmss_to_secs("bogus"), None);
    }
}
