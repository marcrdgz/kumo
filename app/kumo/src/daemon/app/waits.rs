//! Server-owned waiters for agent orchestration primitives.
//!
//! `kumo agent wait` and `kumo pane wait-output` are *server-owned,
//! event-driven* waits: the CLI holds its socket open and the daemon replies
//! once (or on timeout / replacement). No polling loop on the client.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use kumo_protocol::{AgentStatus, AgentWaitKind, DaemonEvent};
use regex::Regex;

/// One `kumo agent wait <pane> --until <kind>` waiter.
#[derive(Debug)]
#[allow(dead_code)]
struct AgentWaiter {
    id: u64,
    client_id: usize,
    session: String,
    pane_id: u64,
    until: AgentWaitKind,
    deadline: Option<Instant>,
    pinned_pid: Option<u32>,
}

/// One `kumo pane wait-output <pane> --regex <pat>` waiter.
#[derive(Debug)]
#[allow(dead_code)]
struct OutputWaiter {
    id: u64,
    client_id: usize,
    session: String,
    pane_id: u64,
    pattern: String,
    is_regex: bool,
    regex: Option<Regex>,
    deadline: Option<Instant>,
    pinned_pid: Option<u32>,
}

/// Registry of all pending server-owned waits.
pub struct WaitRegistry {
    next_id: u64,
    agent: HashMap<u64, AgentWaiter>,
    output: HashMap<u64, OutputWaiter>,
}

impl WaitRegistry {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            agent: HashMap::new(),
            output: HashMap::new(),
        }
    }

    /// Add an agent-status waiter. Returns its id. The caller must have
    /// validated that the pane exists and handled the `agent_blocked` immediate
    /// case before inserting.
    pub fn add_agent_wait(
        &mut self,
        client_id: usize,
        session: String,
        pane_id: u64,
        until: AgentWaitKind,
        timeout_ms: Option<u64>,
        pinned_pid: Option<u32>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
        self.agent.insert(
            id,
            AgentWaiter {
                id,
                client_id,
                session,
                pane_id,
                until,
                deadline,
                pinned_pid,
            },
        );
        id
    }

    /// Add an output waiter. Returns its id or an error string for bad regex.
    #[allow(clippy::too_many_arguments)]
    pub fn add_output_wait(
        &mut self,
        client_id: usize,
        session: String,
        pane_id: u64,
        pattern: String,
        is_regex: bool,
        timeout_ms: Option<u64>,
        pinned_pid: Option<u32>,
    ) -> Result<u64, String> {
        let regex = if is_regex {
            match Regex::new(&pattern) {
                Ok(r) => Some(r),
                Err(e) => return Err(format!("bad regex {pattern:?}: {e}")),
            }
        } else {
            None
        };
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
        self.output.insert(
            id,
            OutputWaiter {
                id,
                client_id,
                session,
                pane_id,
                pattern,
                is_regex,
                regex,
                deadline,
                pinned_pid,
            },
        );
        Ok(id)
    }

    /// Remove all waiters owned by `client_id` (on Detach / disconnect).
    pub fn cancel_client(&mut self, client_id: usize) {
        self.agent.retain(|_, w| w.client_id != client_id);
        self.output.retain(|_, w| w.client_id != client_id);
    }

    /// Remove all waiters targeting `pane_id` (pane closed).
    #[allow(dead_code)]
    pub fn cancel_pane(&mut self, pane_id: u64) {
        self.agent.retain(|_, w| w.pane_id != pane_id);
        self.output.retain(|_, w| w.pane_id != pane_id);
    }

    /// Evaluate agent waiters for `pane_id` whose status just changed to `status`.
    /// Returns `(client_id, DaemonEvent)` pairs to send. Handles occupant pinning.
    pub fn poll_agent(
        &mut self,
        pane_id: u64,
        status: AgentStatus,
        current_pid: Option<u32>,
    ) -> Vec<(usize, DaemonEvent)> {
        let mut done_ids = Vec::new();
        let mut out = Vec::new();
        for (id, w) in &self.agent {
            if w.pane_id != pane_id {
                continue;
            }
            // Pinned occupant check: if the pane's process changed, the wait fails.
            if let (Some(pinned), Some(cur)) = (w.pinned_pid, current_pid) {
                if pinned != cur {
                    out.push((
                        w.client_id,
                        DaemonEvent::Error {
                            code: "agent_replaced".into(),
                            message: format!("pane {pane_id} occupant changed (was {pinned}, now {cur})"),
                        },
                    ));
                    done_ids.push(*id);
                    continue;
                }
            } else if w.pinned_pid.is_some() && current_pid.is_none() {
                out.push((
                    w.client_id,
                    DaemonEvent::Error {
                        code: "agent_replaced".into(),
                        message: format!("pane {pane_id} occupant unknown"),
                    },
                ));
                done_ids.push(*id);
                continue;
            }
            if w.until.matches(status) {
                out.push((
                    w.client_id,
                    DaemonEvent::AgentWaitResult { pane_id, status },
                ));
                done_ids.push(*id);
            }
        }
        for id in done_ids {
            self.agent.remove(&id);
        }
        out
    }

    /// Evaluate output waiters for `pane_id` against `text` (recent tail + visible).
    /// Returns `(client_id, DaemonEvent::PaneWaitResult|Error)` for matches /
    /// replaced occupant.
    pub fn poll_output(
        &mut self,
        pane_id: u64,
        text: &str,
        current_pid: Option<u32>,
    ) -> Vec<(usize, DaemonEvent)> {
        let mut done_ids = Vec::new();
        let mut out = Vec::new();
        for (id, w) in &self.output {
            if w.pane_id != pane_id {
                continue;
            }
            if let (Some(pinned), Some(cur)) = (w.pinned_pid, current_pid) {
                if pinned != cur {
                    out.push((
                        w.client_id,
                        DaemonEvent::Error {
                            code: "agent_replaced".into(),
                            message: format!("pane {pane_id} occupant changed (was {pinned}, now {cur})"),
                        },
                    ));
                    done_ids.push(*id);
                    continue;
                }
            }
            let matched = if w.is_regex {
                if let Some(re) = &w.regex {
                    re.find(text).map(|m| m.as_str().to_string())
                } else {
                    None
                }
            } else if text.contains(&w.pattern) {
                Some(w.pattern.clone())
            } else {
                None
            };
            if let Some(m) = matched {
                out.push((
                    w.client_id,
                    DaemonEvent::PaneWaitResult {
                        pane_id,
                        matched: m,
                    },
                ));
                done_ids.push(*id);
            }
        }
        for id in done_ids {
            self.output.remove(&id);
        }
        out
    }

    /// Sweep timeouts. Call every tick. Returns `(client_id, DaemonEvent::Error{timeout})`.
    pub fn poll_timeouts(&mut self) -> Vec<(usize, DaemonEvent)> {
        let now = Instant::now();
        let mut done_agent = Vec::new();
        let mut done_output = Vec::new();
        let mut out = Vec::new();
        for (id, w) in &self.agent {
            if let Some(dl) = w.deadline {
                if now >= dl {
                    out.push((
                        w.client_id,
                        DaemonEvent::Error {
                            code: "timeout".into(),
                            message: format!(
                                "timed out waiting for pane {} to become {}",
                                w.pane_id,
                                w.until.label()
                            ),
                        },
                    ));
                    done_agent.push(*id);
                }
            }
        }
        for (id, w) in &self.output {
            if let Some(dl) = w.deadline {
                if now >= dl {
                    let pat = if w.is_regex { format!("regex {:?}", w.pattern) } else { format!("{:?}", w.pattern) };
                    out.push((
                        w.client_id,
                        DaemonEvent::Error {
                            code: "timeout".into(),
                            message: format!("timed out waiting for output {pat} in pane {}", w.pane_id),
                        },
                    ));
                    done_output.push(*id);
                }
            }
        }
        for id in done_agent {
            self.agent.remove(&id);
        }
        for id in done_output {
            self.output.remove(&id);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.agent.is_empty() && self.output.is_empty()
    }

    /// Drain waiters whose pane no longer exists. Returns `(client_id, pane_id)` for callers to error.
    pub fn drain_dead_panes(&mut self, live: &std::collections::HashSet<u64>) -> Vec<(usize, u64)> {
        let mut dead = Vec::new();
        let mut remove_agent = Vec::new();
        for (id, w) in &self.agent {
            if !live.contains(&w.pane_id) {
                dead.push((w.client_id, w.pane_id));
                remove_agent.push(*id);
            }
        }
        let mut remove_output = Vec::new();
        for (id, w) in &self.output {
            if !live.contains(&w.pane_id) {
                dead.push((w.client_id, w.pane_id));
                remove_output.push(*id);
            }
        }
        for id in remove_agent {
            self.agent.remove(&id);
        }
        for id in remove_output {
            self.output.remove(&id);
        }
        dead
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_wait_matches_only_until() {
        let mut r = WaitRegistry::new();
        r.add_agent_wait(1, "s".into(), 42, AgentWaitKind::Blocked, None, None);
        // Not blocked -> no result
        assert!(r.poll_agent(42, AgentStatus::Idle, None).is_empty());
        assert_eq!(r.agent.len(), 1);
        // Blocked -> resolved
        let out = r.poll_agent(42, AgentStatus::Blocked, None);
        assert_eq!(out.len(), 1);
        assert_eq!(r.agent.len(), 0);
    }

    #[test]
    fn output_wait_regex_match() {
        let mut r = WaitRegistry::new();
        r.add_output_wait(1, "s".into(), 10, "passed|failed".into(), true, None, None).unwrap();
        assert!(r.poll_output(10, "all good", None).is_empty());
        let out = r.poll_output(10, "test passed", None);
        assert_eq!(out.len(), 1);
        match &out[0].1 {
            DaemonEvent::PaneWaitResult { matched, .. } => assert_eq!(matched, "passed"),
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn pinned_pid_replacement_fails_wait() {
        let mut r = WaitRegistry::new();
        r.add_agent_wait(1, "s".into(), 7, AgentWaitKind::Idle, None, Some(100));
        let out = r.poll_agent(7, AgentStatus::Idle, Some(101));
        assert_eq!(out.len(), 1);
        match &out[0].1 {
            DaemonEvent::Error { code, .. } => assert_eq!(code, "agent_replaced"),
            _ => panic!(),
        }
    }

    #[test]
    fn timeout_sweeps() {
        let mut r = WaitRegistry::new();
        r.add_agent_wait(1, "s".into(), 1, AgentWaitKind::Idle, Some(0), None);
        // 0 ms timeout is already expired
        std::thread::sleep(std::time::Duration::from_millis(1));
        let out = r.poll_timeouts();
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0].1, DaemonEvent::Error { code, .. } if code=="timeout"));
    }

    #[test]
    fn cancel_client_drops_both() {
        let mut r = WaitRegistry::new();
        r.add_agent_wait(1, "s".into(), 1, AgentWaitKind::Blocked, None, None);
        r.add_output_wait(1, "s".into(), 1, "hi".into(), false, None, None).unwrap();
        r.add_agent_wait(2, "s".into(), 2, AgentWaitKind::Idle, None, None);
        r.cancel_client(1);
        assert_eq!(r.agent.len(), 1);
        assert_eq!(r.output.len(), 0);
    }
}
