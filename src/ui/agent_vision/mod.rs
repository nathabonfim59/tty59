use std::cmp::Reverse;
use std::collections::HashMap;

use tty59_core::core::cli_agent::{AgentOverview, AgentSessionState, AgentStatus, CLIAgent};
use tty59_core::core::machine::{PaneRecord, TabId};
use tty59_core::core::session::WorkspaceId;
use tty59_core::host::HostId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct PaneKey {
    pub host: HostId,
    pub pane: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PaneLocation {
    pub key: PaneKey,
    pub workspace: WorkspaceId,
    pub tab: TabId,
    pub tab_title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Idle,
    Working,
    Waiting,
    Done,
    Exited,
}

impl From<AgentStatus> for State {
    fn from(status: AgentStatus) -> Self {
        match status {
            AgentStatus::Idle => Self::Idle,
            AgentStatus::Working => Self::Working,
            AgentStatus::Waiting => Self::Waiting,
            AgentStatus::Done => Self::Done,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PaneSummary {
    pub location: PaneLocation,
    pub agent: Option<CLIAgent>,
    pub kind: String,
    pub state: State,
    pub since_ms: Option<u64>,
    epoch: Option<u64>,
    turns: u64,
    unseen_done: bool,
}

impl PaneSummary {
    pub fn from_live(
        location: PaneLocation,
        agent: Option<CLIAgent>,
        session: Option<&AgentSessionState>,
        unseen_done: bool,
    ) -> Option<Self> {
        if let Some(overview) = session.and_then(AgentSessionState::overview) {
            return Some(Self::from_overview(location, agent, overview, unseen_done));
        }
        let agent = agent?;
        Some(Self {
            location,
            agent: Some(agent),
            kind: agent.slug().into(),
            state: session
                .map_or(AgentStatus::Idle, |state| state.status)
                .into(),
            since_ms: session.and_then(|state| state.since_ms),
            epoch: None,
            turns: session.map_or(0, |state| state.turns),
            unseen_done,
        })
    }

    pub fn from_mirror(
        location: PaneLocation,
        record: &PaneRecord,
        unseen_done: bool,
    ) -> Option<Self> {
        if !record.live {
            return None;
        }
        if let Some(overview) = &record.agent_overview {
            let agent = record.agent.as_ref().map(|facts| facts.agent);
            return Some(Self::from_overview(
                location,
                agent,
                overview.clone(),
                unseen_done,
            ));
        }
        let facts = record.agent.as_ref()?;
        Some(Self {
            location,
            agent: Some(facts.agent),
            kind: facts.agent.slug().into(),
            state: facts.status.unwrap_or_default().into(),
            since_ms: None,
            epoch: None,
            turns: 0,
            unseen_done,
        })
    }

    fn from_overview(
        location: PaneLocation,
        agent: Option<CLIAgent>,
        overview: AgentOverview,
        unseen_done: bool,
    ) -> Self {
        Self {
            location,
            agent: agent.or_else(|| CLIAgent::from_slug(&overview.kind)),
            kind: overview.kind,
            state: if overview.exited {
                State::Exited
            } else {
                overview.status.into()
            },
            since_ms: overview.since_ms,
            epoch: Some(overview.epoch),
            turns: overview.turns,
            unseen_done,
        }
    }

    pub fn agent_name(&self) -> &str {
        match self.agent {
            Some(agent) => agent.display_name(),
            None => &self.kind,
        }
    }

    fn event(&self) -> Option<AttentionEvent> {
        let kind = match self.state {
            State::Waiting => Attention::Waiting,
            State::Done => Attention::Finished,
            _ => return None,
        };
        Some(AttentionEvent {
            kind,
            epoch: self.epoch,
            since_ms: self.since_ms,
            turns: self.turns,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Attention {
    Waiting,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AttentionEvent {
    kind: Attention,
    epoch: Option<u64>,
    since_ms: Option<u64>,
    turns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Project {
    pub id: WorkspaceId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct AttentionCount {
    pub waiting: usize,
    pub finished: usize,
}

impl AttentionCount {
    pub fn total(self) -> usize {
        self.waiting + self.finished
    }
}

pub(super) struct WorkspaceSummary<'a> {
    pub project: &'a Project,
    pub needs_attention: AttentionCount,
}

#[derive(Default)]
pub(super) struct InboxReadState {
    read: HashMap<PaneKey, AttentionEvent>,
}

pub(super) struct InboxRows<'a> {
    pub attention: Vec<&'a PaneSummary>,
    pub other: Vec<&'a PaneSummary>,
}

impl InboxRows<'_> {
    pub fn keep_selection(&self, selected: Option<PaneKey>) -> Option<PaneKey> {
        let rows = self.attention.iter().chain(&self.other);
        if let Some(key) = selected
            && rows.clone().any(|row| row.location.key == key)
        {
            return Some(key);
        }
        rows.map(|row| row.location.key).next()
    }
}

impl InboxReadState {
    /// A fresh event or a known return to work replaces the read mark. A pane
    /// absent from a partial snapshot may still be running on a disconnected host.
    pub fn observe(&mut self, panes: &[PaneSummary]) {
        for pane in panes {
            let key = pane.location.key;
            if pane.state == State::Done && !pane.unseen_done {
                self.mark_read(pane);
            } else if self
                .read
                .get(&key)
                .is_some_and(|seen| Some(*seen) != pane.event())
            {
                self.read.remove(&key);
            }
        }
    }

    pub fn mark_read(&mut self, pane: &PaneSummary) {
        if let Some(event) = pane.event() {
            self.read.insert(pane.location.key, event);
        }
    }

    fn attention(&self, pane: &PaneSummary) -> Option<AttentionEvent> {
        if pane.state == State::Done && !pane.unseen_done {
            return None;
        }
        pane.event()
            .filter(|event| self.read.get(&pane.location.key) != Some(event))
    }

    pub fn rows<'a>(
        &self,
        panes: &'a [PaneSummary],
        project: Option<WorkspaceId>,
    ) -> InboxRows<'a> {
        let mut attention = Vec::new();
        let mut other = Vec::new();
        for pane in panes.iter().filter(|pane| {
            pane.state != State::Exited && project.is_none_or(|id| pane.location.workspace == id)
        }) {
            if let Some(event) = self.attention(pane) {
                attention.push((event.kind, pane));
            } else {
                other.push(pane);
            }
        }
        let recency = |pane: &PaneSummary| {
            (
                Reverse(pane.since_ms),
                pane.location.key.host.0,
                pane.location.key.pane,
            )
        };
        attention.sort_by_key(|(kind, pane)| (*kind, recency(pane)));
        other.sort_by_key(|pane| recency(pane));
        InboxRows {
            attention: attention.into_iter().map(|(_, pane)| pane).collect(),
            other,
        }
    }

    pub fn workspaces<'a>(
        &self,
        projects: &'a [Project],
        panes: &[PaneSummary],
    ) -> Vec<WorkspaceSummary<'a>> {
        let mut counts = HashMap::<WorkspaceId, AttentionCount>::new();
        for pane in panes {
            if let Some(event) = self.attention(pane) {
                let count = counts.entry(pane.location.workspace).or_default();
                match event.kind {
                    Attention::Waiting => count.waiting += 1,
                    Attention::Finished => count.finished += 1,
                }
            }
        }
        projects
            .iter()
            .map(|project| WorkspaceSummary {
                project,
                needs_attention: counts.get(&project.id).copied().unwrap_or_default(),
            })
            .collect()
    }

    pub fn handle_and_advance(
        &mut self,
        panes: &[PaneSummary],
        current: Option<PaneKey>,
    ) -> Option<PaneKey> {
        if let Some(current) = current
            && let Some(pane) = panes.iter().find(|pane| pane.location.key == current)
        {
            self.mark_read(pane);
        }
        self.rows(panes, None)
            .attention
            .first()
            .map(|pane| pane.location.key)
    }
}

#[cfg(test)]
mod tests;
