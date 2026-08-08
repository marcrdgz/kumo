use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::pty::{Pty, PtySpec};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PaneInfo {
    pub pane_id: u64,
    pub cols: u16,
    pub rows: u16,
    pub shell: String,
    pub ai: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: u64,
    pub name: String,
    pub panes: Vec<PaneInfo>,
    pub active_pane: u64,
}

pub struct Pane {
    pub id: u64,
    pub pty: Pty,
    pub parent: Option<u64>,
    pub children: Vec<u64>,
    pub ai: bool,
}

pub struct Session {
    pub id: u64,
    pub name: String,
    pub panes: HashMap<u64, Pane>,
    pub active_pane: u64,
}

/// Global application state holding all sessions (the multiplexer daemon).
pub struct AppState {
    pub sessions: Mutex<HashMap<u64, Session>>,
    pub next_session_id: Mutex<u64>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next_session_id: Mutex::new(1),
        }
    }

    fn alloc_session_id(&self) -> u64 {
        let mut id = self.next_session_id.lock().unwrap();
        let current = *id;
        *id += 1;
        current
    }

    pub fn create_session(&self, name: &str, shell: &str, cwd: Option<&str>, cols: u16, rows: u16) -> anyhow::Result<SessionInfo> {
        let id = self.alloc_session_id();
        let pane_id = Pty::next_pane_id();

        let mut pty = Pty::spawn(&PtySpec {
            shell: shell.to_string(),
            program: None,
            cwd: cwd.map(std::path::PathBuf::from),
            cols,
            rows,
        })?;
        pty.id = pane_id;

        let pane = Pane {
            id: pane_id,
            pty,
            parent: None,
            children: Vec::new(),
            ai: false,
        };

        let session = Session {
            id,
            name: if name.is_empty() {
                format!("session-{id}")
            } else {
                name.to_string()
            },
            panes: HashMap::from([(pane_id, pane)]),
            active_pane: pane_id,
        };

        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(id, session);

        Ok(Self::info(sessions.get(&id).unwrap()))
    }

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.lock().unwrap();
        let mut out: Vec<SessionInfo> = sessions.values().map(Self::info).collect();
        out.sort_by_key(|s| s.session_id);
        out
    }

    pub fn get_session(&self, session_id: u64) -> Option<SessionInfo> {
        let sessions = self.sessions.lock().unwrap();
        sessions.get(&session_id).map(Self::info)
    }

    /// Spawn a new pane in a session. `direction` describes the split axis
    /// relative to the active pane (e.g. "h" or "v"). Returns the new pane.
    pub fn split_pane(
        &self,
        session_id: u64,
        shell: &str,
        program: Option<(String, Vec<String>)>,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        direction: &str,
        ai: bool,
    ) -> anyhow::Result<PaneInfo> {
        let _ = direction;
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;

        let pane_id = Pty::next_pane_id();
        let mut pty = Pty::spawn(&PtySpec {
            shell: shell.to_string(),
            program,
            cwd: cwd.map(std::path::PathBuf::from),
            cols,
            rows,
        })?;
        pty.id = pane_id;

        let parent = session.active_pane;
        let pane = Pane {
            id: pane_id,
            pty,
            parent: Some(parent),
            children: Vec::new(),
            ai,
        };

        // Wire up tree relationship to the parent pane.
        if let Some(parent_pane) = session.panes.get_mut(&parent) {
            parent_pane.children.push(pane_id);
        }

        session.panes.insert(pane_id, pane);
        session.active_pane = pane_id;

        Ok(Self::pane_info(&session.panes[&pane_id]))
    }

    /// Spawn a new pane running an AI CLI program (e.g. `opencode`).
    pub fn spawn_ai_pane(
        &self,
        session_id: u64,
        program: &str,
        args: &[String],
        cwd: std::path::PathBuf,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<PaneInfo> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;

        let pane_id = Pty::next_pane_id();
        let mut pty = Pty::spawn(&PtySpec {
            shell: String::new(),
            program: Some((program.to_string(), args.to_vec())),
            cwd: Some(cwd),
            cols,
            rows,
        })?;
        pty.id = pane_id;

        let parent = session.active_pane;
        let pane = Pane {
            id: pane_id,
            pty,
            parent: Some(parent),
            children: Vec::new(),
            ai: true,
        };

        if let Some(parent_pane) = session.panes.get_mut(&parent) {
            parent_pane.children.push(pane_id);
        }

        session.panes.insert(pane_id, pane);
        session.active_pane = pane_id;

        Ok(Self::pane_info(&session.panes[&pane_id]))
    }

    pub fn write_pane(&self, session_id: u64, pane_id: u64, data: &[u8]) -> anyhow::Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        let pane = session
            .panes
            .get_mut(&pane_id)
            .ok_or_else(|| anyhow::anyhow!("pane not found"))?;
        pane.pty.write(data)
    }

    pub fn resize_pane(&self, session_id: u64, pane_id: u64, cols: u16, rows: u16) -> anyhow::Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        let pane = session
            .panes
            .get_mut(&pane_id)
            .ok_or_else(|| anyhow::anyhow!("pane not found"))?;
        pane.pty.resize(cols, rows)
    }

    pub fn focus_pane(&self, session_id: u64, pane_id: u64) -> anyhow::Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        if !session.panes.contains_key(&pane_id) {
            return Err(anyhow::anyhow!("pane not found"));
        }
        session.active_pane = pane_id;
        Ok(())
    }

    /// Kill and remove a pane. Returns true if the session was removed (last pane).
    pub fn close_pane(&self, session_id: u64, pane_id: u64) -> anyhow::Result<bool> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;

        let mut pane = session
            .panes
            .remove(&pane_id)
            .ok_or_else(|| anyhow::anyhow!("pane not found"))?;
        pane.pty.kill();

        // Remove from parent's children list.
        if let Some(parent_id) = pane.parent {
            if let Some(parent) = session.panes.get_mut(&parent_id) {
                parent.children.retain(|c| *c != pane_id);
            }
        }

        if session.panes.is_empty() {
            sessions.remove(&session_id);
            return Ok(true);
        }

        if session.active_pane == pane_id {
            let next = session
                .panes
                .keys()
                .next()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("no panes left"))?;
            session.active_pane = next;
        }

        Ok(false)
    }

    pub fn close_session(&self, session_id: u64) -> anyhow::Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut session = sessions
            .remove(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        let ids: Vec<u64> = session.panes.keys().copied().collect();
        for id in ids {
            if let Some(mut pane) = session.panes.remove(&id) {
                pane.pty.kill();
            }
        }
        Ok(())
    }

    /// Return the PID of the child process running inside a pane (the shell,
    /// or the AI CLI for AI panes).
    pub fn pane_child_pid(&self, session_id: u64, pane_id: u64) -> anyhow::Result<u32> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        let pane = session
            .panes
            .get(&pane_id)
            .ok_or_else(|| anyhow::anyhow!("pane not found"))?;
        pane.pty
            .child
            .process_id()
            .ok_or_else(|| anyhow::anyhow!("no child process"))
    }

    /// The shell program used to spawn a pane, or the AI program for AI panes.
    pub fn pane_shell(&self, session_id: u64, pane_id: u64) -> anyhow::Result<String> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        let pane = session
            .panes
            .get(&pane_id)
            .ok_or_else(|| anyhow::anyhow!("pane not found"))?;
        Ok(pane.pty.shell.clone())
    }

    /// Detach a pane's read loop onto a background thread. The callback fires
    /// for each chunk of output with the pane id.
    pub fn detach_read_loop(&self, session_id: u64, pane_id: u64, cb: impl Fn(u64, Vec<u8>) + Send + 'static) -> anyhow::Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        let pane = session
            .panes
            .get_mut(&pane_id)
            .ok_or_else(|| anyhow::anyhow!("pane not found"))?;

        let reader = pane.pty.master.try_clone_reader()?;
        Pty::read_loop(reader, move |data| cb(pane_id, data));
        Ok(())
    }

    fn pane_info(pane: &Pane) -> PaneInfo {
        PaneInfo {
            pane_id: pane.id,
            cols: pane.pty.cols,
            rows: pane.pty.rows,
            shell: pane.pty.shell.clone(),
            ai: pane.ai,
        }
    }

    fn info(session: &Session) -> SessionInfo {
        let mut panes: Vec<PaneInfo> = session.panes.values().map(Self::pane_info).collect();
        panes.sort_by_key(|p| p.pane_id);
        SessionInfo {
            session_id: session.id,
            name: session.name.clone(),
            panes,
            active_pane: session.active_pane,
        }
    }
}

#[derive(Deserialize)]
pub struct SpawnRequest {
    pub name: Option<String>,
    pub shell: Option<String>,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitRequest {
    pub session_id: u64,
    pub shell: Option<String>,
    pub program: Option<String>,
    pub args: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub direction: String,
    #[serde(default)]
    pub ai: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneRequest {
    pub session_id: u64,
    pub pane_id: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPaneRequest {
    pub session_id: u64,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeRequest {
    pub session_id: u64,
    pub pane_id: u64,
    pub cols: u16,
    pub rows: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn spawn_ai_pane_runs_program() {
        let state = AppState::new();
        state
            .create_session("test", "/bin/sh", None, 80, 24)
            .expect("create session");
        let info = state
            .spawn_ai_pane(
                1,
                "/bin/sh",
                &["-c".into(), "echo AI_PANE_OK".into()],
                std::path::PathBuf::from("/tmp"),
                80,
                24,
            )
            .expect("spawn ai pane");
        assert_eq!(info.shell, "/bin/sh");

        let mut sessions = state.sessions.lock().unwrap();
        let session = sessions.get_mut(&1).unwrap();
        let pane = session.panes.get_mut(&info.pane_id).unwrap();
        let reader = pane.pty.master.try_clone_reader().expect("clone reader");
        let (tx, rx) = mpsc::channel();
        Pty::read_loop(reader, move |data| {
            let _ = tx.send(data);
        });
        let got = rx.recv_timeout(Duration::from_secs(5)).expect("output");
        let text = String::from_utf8_lossy(&got);
        assert!(
            text.contains("AI_PANE_OK"),
            "unexpected output: {text:?}"
        );
    }
}
