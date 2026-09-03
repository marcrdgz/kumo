use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use kumo_protocol::TimelineRecord;

const CAP_PER_PANE: usize = 200;
const CAP_OUTPUT: usize = 16 * 1024;

#[derive(Default)]
pub struct Timeline {
    per_pane: HashMap<u64, VecDeque<TimelineRecord>>,
    // Last seen prompt block hash per pane to avoid duplicates
    last_hash: HashMap<u64, u64>,
}

impl Timeline {
    pub fn push_if_new(&mut self, pane_id: u64, session: String, prompt: String, output: String, cwd: PathBuf) {
        let mut hash = 0u64;
        for b in prompt.bytes().chain(output.bytes()) {
            hash = hash.wrapping_mul(31).wrapping_add(b as u64);
        }
        if self.last_hash.get(&pane_id) == Some(&hash) {
            return;
        }
        self.last_hash.insert(pane_id, hash);
        let mut output_capped = output;
        if output_capped.len() > CAP_OUTPUT {
            output_capped.truncate(CAP_OUTPUT);
            output_capped.push_str("\n…[truncated]");
        }
        let mut prompt_capped = prompt;
        if prompt_capped.len() > 1024 {
            prompt_capped.truncate(1024);
        }
        let ts_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
        let rec = TimelineRecord {
            pane_id,
            session: session.clone(),
            prompt: prompt_capped,
            output: output_capped,
            cwd,
            ts_ms,
        };
        let q = self.per_pane.entry(pane_id).or_default();
        q.push_back(rec);
        while q.len() > CAP_PER_PANE {
            q.pop_front();
        }
    }

    pub fn list(&self, session: Option<&str>, pane_id: Option<u64>, query: Option<&str>) -> Vec<TimelineRecord> {
        let mut out = Vec::new();
        for (pid, deque) in &self.per_pane {
            if let Some(filter_pid) = pane_id {
                if *pid != filter_pid {
                    continue;
                }
            }
            for rec in deque {
                if let Some(sess) = session {
                    if rec.session != sess {
                        continue;
                    }
                }
                if let Some(q) = query {
                    let ql = q.to_ascii_lowercase();
                    if !rec.prompt.to_ascii_lowercase().contains(&ql) && !rec.output.to_ascii_lowercase().contains(&ql) && !rec.cwd.to_string_lossy().to_ascii_lowercase().contains(&ql) {
                        continue;
                    }
                }
                out.push(rec.clone());
            }
        }
        out.sort_by_key(|r| r.ts_ms);
        out
    }
}
