//! Bind durable execution identities to this daemon's live panes.

use anyhow::{bail, Result};
use kumo_core::ade::{InboxKind, RunState};
use kumo_protocol::{AdeInboxItem, AdeRun, AdeWorkspace, DaemonEvent};

use super::App;
use crate::daemon::agents::AgentStatus;

impl App {
    pub(super) fn close_unbound_ade_runs(&mut self) -> Result<()> {
        let stale: Vec<_> = self.ade.snapshot().runs().iter()
            .filter(|run| run.state != RunState::Closed && !self.ade_runs.values().any(|id| *id == run.id))
            .map(|run| run.id).collect();
        for id in stale {
            self.ade.observe(id, RunState::Closed, None, None)?;
        }
        Ok(())
    }

    /// Called after metadata refresh, at most four times per second. Workspace
    /// canonicalization is cached; persistence runs on its own worker.
    pub(super) fn refresh_ade(&mut self) -> Result<()> {
        self.last_ade_scan = std::time::Instant::now();
        for session in &self.sessions {
            let cached = self.ade_workspaces.get(&session.workspace);
            if cached.is_none() {
                let id = self.ade.register_workspace(&session.workspace, &session.name)?;
                self.ade_workspaces.insert(session.workspace.clone(), (session.name.clone(), id));
            }
        }
        let live: Vec<_> = self.panes.iter()
            .filter(|(_, pane)| pane.is_ai_cli() && !pane.dead)
            .map(|(&id, pane)| (id, pane.cwd.clone(), pane.pty.process_id(), self.agent_label(id), self.current_agent_status(id)))
            .collect();
        let stale: Vec<_> = self.ade_runs.keys().copied()
            .filter(|id| !live.iter().any(|(pane, ..)| pane == id)).collect();
        for pane in stale {
            if let Some(run) = self.ade_runs.remove(&pane) {
                self.ade.observe(run, RunState::Closed, Some("closed".into()), Some(InboxKind::Completed))?;
            }
        }
        for (pane, path, pid, agent, status) in live {
            let run = if let Some(id) = self.ade_runs.get(&pane) {
                *id
            } else {
                let workspace = if let Some((_, id)) = self.ade_workspaces.get(&path) {
                    *id
                } else {
                    let label = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let id = self.ade.register_workspace(&path, &label)?;
                    self.ade_workspaces.insert(path, (label, id));
                    id
                };
                let id = self.ade.start_run(workspace, pane, pid, agent, String::new())?;
                self.ade_runs.insert(pane, id);
                id
            };
            let state = match status.unwrap_or(AgentStatus::Unknown) {
                AgentStatus::Working => RunState::Working,
                AgentStatus::Blocked => RunState::Blocked,
                AgentStatus::Idle => RunState::Idle,
                AgentStatus::Done => RunState::Done,
                AgentStatus::Unknown => RunState::Unknown,
            };
            let kind = match state {
                RunState::Blocked => Some(InboxKind::Blocked),
                RunState::Done => Some(InboxKind::Completed),
                _ => None,
            };
            // Repeated observations are ignored by observe. A later actionable
            // transition gets a new key, including after an exec restart.
            let key = kind.as_ref().map(|_| format!("transition:{}", self.ade.snapshot().inbox().iter().filter(|item| item.run_id == run).count()));
            self.ade.observe(run, state, key, kind)?;
        }
        Ok(())
    }

    pub(super) fn ade_event(&self) -> DaemonEvent {
        let store = self.ade.snapshot();
        DaemonEvent::AdeSnapshot {
            workspaces: store.workspaces().iter().map(|w| AdeWorkspace { id: w.id, path: w.path.clone(), label: w.label.clone() }).collect(),
            runs: store.runs().iter().map(|r| AdeRun {
                id: r.id, workspace_id: r.workspace_id, pane_id: r.pane_id, agent: r.agent.clone(),
                state: match r.state {
                    RunState::Submitted => "submitted", RunState::Working => "working",
                    RunState::Blocked => "blocked", RunState::Idle => "idle",
                    RunState::Done => "done", RunState::Unknown => "unknown", RunState::Closed => "closed",
                }.into(),
            }).collect(),
            inbox: store.inbox().iter().map(|i| AdeInboxItem {
                id: i.id, run_id: i.run_id, key: i.key.clone(), created_at_ms: i.created_at_ms, acknowledged: i.acknowledged,
                kind: match i.kind {
                    InboxKind::Approval => "approval", InboxKind::Question => "question",
                    InboxKind::Blocked => "blocked", InboxKind::Completed => "completed", InboxKind::Review => "review",
                }.into(),
            }).collect(),
        }
    }

    pub(super) fn focus_ade_run(&mut self, run_id: u64) -> Result<()> {
        let pane = self.ade_runs.iter().find_map(|(&pane, &id)| (id == run_id).then_some(pane));
        let Some(pane) = pane.filter(|id| self.panes.get(id).is_some_and(|pane| !pane.dead)) else {
            bail!("run {run_id} has no live terminal");
        };
        let session = self.sessions.iter().find(|s| s.find_tab_containing(pane).is_some()).map(|s| s.name.clone());
        if !session.is_some_and(|name| self.focus_pane_in_session(&name, pane)) {
            bail!("run {run_id} has no visible terminal");
        }
        Ok(())
    }
}
