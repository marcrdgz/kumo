//! Per-process CPU/RAM sampling for the sidebar's agent micro-pill metrics.
//!
//! The daemon owns every pane's process tree, so it can report live numbers
//! instead of shipping placeholder text. CPU is measured over a *sliding
//! window* (the last `WINDOW` samples, ~6 s at the 500 ms agent-status
//! refresh): the CPU consumed across the window divided by its wall-clock
//! length, so a bursty agent (a TUI that idles between work pulses) still
//! reads a truthful average instead of a 0 to 100% random guess between
//! consecutive samples.
//!
//! Sampling runs only for AI panes (usually a handful), via `ps` on macOS and
//! `/proc` on Linux. Any failure yields `(0.0, 0)` and the PID's state is
//! dropped, so a missing process is never mistaken for a busy one.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::Instant;

/// How many consecutive samples to retain per PID (at the 500 ms agent-status
/// refresh this is ~6 s of history).
const WINDOW: usize = 12;

/// Short history of CPU-time snapshots for one process.
#[derive(Default)]
struct PidSamples {
    snapshots: VecDeque<(f64, Instant)>,
    rss_kb: u64,
}

/// Stateful sampler: keeps a sliding window of CPU-time snapshots per PID so
/// each call returns the CPU consumed across the window (see module docs).
#[derive(Default)]
pub struct ProcSampler {
    samples: HashMap<u32, PidSamples>,
}

impl ProcSampler {
    /// Sample `pid`'s CPU% (of one core) across the sliding window and its
    /// resident memory in KiB. `(0.0, 0)` when the process is gone or the
    /// platform cannot read it.
    pub fn sample(&mut self, pid: u32) -> (f32, u64) {
        let (cpu_secs, rss_kb) = match cpu_time_rss(pid) {
            Some(v) => v,
            None => {
                self.samples.remove(&pid);
                return (0.0, 0);
            }
        };
        let now = Instant::now();
        let cpu = {
            let s = self.samples.entry(pid).or_default();
            s.snapshots.push_back((cpu_secs, now));
            if s.snapshots.len() > WINDOW {
                s.snapshots.pop_front();
            }
            s.rss_kb = rss_kb;
            window_cpu(&s.snapshots, now)
        };
        (cpu.clamp(0.0, 999.0), rss_kb)
    }

    /// Drop any state for `pid` (pane closed / agent killed), so the next
    /// sample starts fresh instead of reporting a huge "usage" spike.
    pub fn forget(&mut self, pid: u32) {
        self.samples.remove(&pid);
    }
}

/// CPU% across the retained window: the CPU time consumed from the oldest
/// snapshot to `now` over the wall-clock span. The first sample (no window
/// history) reads 0.0.
fn window_cpu(snapshots: &VecDeque<(f64, Instant)>, now: Instant) -> f32 {
    let (Some(&(c0, t0)), Some(&(c1, _))) = (snapshots.front(), snapshots.back()) else {
        return 0.0;
    };
    let wall = now.duration_since(t0).as_secs_f64();
    if wall <= 0.0 {
        return 0.0;
    }
    ((c1 - c0).max(0.0) / wall * 100.0) as f32
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
    use super::*;

    #[test]
    fn window_consumes_cpu_time_half_of_wall_clock() {
        // 0.5 s of CPU over 1.0 s of wall clock = 50% (of one core).
        let now = Instant::now();
        let mut snapshots = VecDeque::new();
        snapshots.push_back((10.0, now - std::time::Duration::from_secs(1)));
        snapshots.push_back((10.5, now));
        assert_eq!(window_cpu(&snapshots, now), 50.0);
        // Bursts between samples vanish inside the window: 1.5 s CPU over
        // 6.0 s wall = 25% — not 0%, not 100%.
        let now2 = now + std::time::Duration::from_secs(5);
        snapshots.push_back((11.5, now2));
        assert_eq!(window_cpu(&snapshots, now2), 25.0);
    }

    #[test]
    fn window_first_sample_is_zero_and_never_negative() {
        let now = Instant::now();
        let mut snapshots = VecDeque::new();
        snapshots.push_back((5.0, now));
        assert_eq!(window_cpu(&snapshots, now), 0.0);
        // cpu clock moving backwards (proc reset) must still read 0.0.
        snapshots.push_back((4.9, now));
        assert_eq!(window_cpu(&snapshots, now), 0.0);
    }

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
