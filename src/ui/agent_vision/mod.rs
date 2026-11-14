use std::cmp::Reverse;
use std::collections::HashMap;

use tty59_core::core::cli_agent::{AgentOverview, AgentSessionState, AgentStatus, CLIAgent};
use tty59_core::core::group_key::GroupKey;
use tty59_core::core::machine::{PaneRecord, TabId};
use tty59_core::core::session::WorkspaceId;
use tty59_core::host::HostId;

mod navigation;
mod render;
mod runtime;
mod summary;
#[cfg(test)]
mod ui_tests;
pub(in crate::ui) use runtime::AgentVision;

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
    pub group: Option<GroupKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PaneTarget {
    pub key: PaneKey,
    pub workspace: WorkspaceId,
    pub tab: TabId,
}

impl From<&PaneLocation> for PaneTarget {
    fn from(location: &PaneLocation) -> Self {
        Self {
            key: location.key,
            workspace: location.workspace,
            tab: location.tab,
        }
    }
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
    /// One-based position among all tab panes, including non-agent panes.
    pub pane_position: Option<(usize, usize)>,
    pub reason: Option<String>,
    pub last_line: Option<String>,
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
            let mut pane = Self::from_overview(location, agent, overview, unseen_done);
            pane.reason = session.and_then(|session| session.message.clone());
            return Some(pane);
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
            pane_position: None,
            reason: session.and_then(|session| session.message.clone()),
            last_line: None,
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
            pane_position: None,
            reason: None,
            last_line: None,
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
            pane_position: None,
            reason: None,
            last_line: None,
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

    /// The tab's label, except that an exited agent's tab has reverted to the
    /// shell's cwd: the agent's own name keeps the row identifiable.
    pub fn title(&self) -> &str {
        match self.state {
            State::Exited => self.agent_name(),
            _ => &self.location.tab_title,
        }
    }

    /// [`title`](Self::title) plus the split marker, for Inbox and Kanban.
    pub fn heading(&self) -> String {
        match self.pane_marker() {
            Some(marker) => format!("{} · {marker}", self.title()),
            None => self.title().to_string(),
        }
    }

    /// The reason without its age, as searched. An exit drops the session's
    /// last message, which described a turn that no longer runs.
    pub fn reason_text(&self) -> &str {
        match self.state {
            State::Exited => self.state.label(),
            _ => self.reason.as_deref().unwrap_or(self.state.label()),
        }
    }

    /// The reason line a row shows; an exit says how long ago it happened.
    pub fn reason_line(&self, now_s: u64) -> String {
        match (self.state, self.since_ms) {
            (State::Exited, Some(since)) => crate::ui::i18n::t_fmt(
                crate::ui::i18n::L10nKey::VisionExitedAge,
                &[(
                    "age",
                    &crate::ui::home::compact_age(now_s.saturating_sub(since / 1000)),
                )],
            ),
            _ => self.reason_text().to_string(),
        }
    }

    pub fn pane_marker(&self) -> Option<String> {
        self.pane_position.map(|(index, total)| {
            crate::ui::i18n::t_fmt(
                crate::ui::i18n::L10nKey::VisionPane,
                &[("i", &index.to_string()), ("n", &total.to_string())],
            )
        })
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

    pub fn matches_search(&self, query: &str, project: &str) -> bool {
        query.split_whitespace().all(|word| {
            [
                self.title(),
                self.agent_name(),
                &self.kind,
                project,
                self.reason_text(),
            ]
            .iter()
            .any(|text| text.to_lowercase().contains(word))
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
    pub group: Option<GroupKey>,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ViewMode {
    #[default]
    Workbench,
    Inbox,
    Kanban,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum FocusZone {
    #[default]
    List,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NavigationKey {
    PreviousProject,
    NextProject,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
}

impl NavigationKey {
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "[" => Self::PreviousProject,
            "]" => Self::NextProject,
            "k" | "up" => Self::Up,
            "j" | "down" => Self::Down,
            "h" | "left" => Self::Left,
            "l" | "right" => Self::Right,
            "enter" => Self::Enter,
            "escape" => Self::Escape,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NavigationResult {
    Unhandled,
    List,
    Terminal(PaneTarget),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum ProjectFilter {
    #[default]
    All,
    Group(Option<GroupKey>),
}

impl ProjectFilter {
    pub fn contains(&self, group: &Option<GroupKey>) -> bool {
        match self {
            Self::All => true,
            Self::Group(selected) => selected == group,
        }
    }

    pub fn panes<'a, 'filter>(
        &'filter self,
        workspace: WorkspaceId,
        panes: &'a [PaneSummary],
    ) -> impl Iterator<Item = &'a PaneSummary> + use<'a, 'filter> {
        panes.iter().filter(move |pane| {
            pane.location.workspace == workspace && self.contains(&pane.location.group)
        })
    }

    pub fn cycle(&self, projects: &[Project], forward: bool) -> Self {
        let current = match self {
            Self::All => 0,
            Self::Group(group) => projects
                .iter()
                .position(|project| &project.group == group)
                .map_or(0, |index| index + 1),
        };
        let count = projects.len() + 1;
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        next.checked_sub(1).map_or(Self::All, |index| {
            Self::Group(projects[index].group.clone())
        })
    }

    pub fn reconcile(&mut self, projects: &[Project]) {
        if let Self::Group(group) = self
            && !projects
                .iter()
                .any(|project| project.group.as_ref() == group.as_ref())
        {
            *self = Self::All;
        }
    }
}

pub(super) struct VisionState {
    workspace: WorkspaceId,
    mode: ViewMode,
    project_filter: ProjectFilter,
    focus: FocusZone,
    selected: Option<PaneKey>,
    pub show_other: bool,
}

impl VisionState {
    pub fn new(workspace: WorkspaceId) -> Self {
        Self {
            workspace,
            mode: ViewMode::default(),
            project_filter: ProjectFilter::default(),
            focus: FocusZone::default(),
            selected: None,
            show_other: false,
        }
    }

    pub fn set_workspace(&mut self, workspace: WorkspaceId) {
        if self.workspace != workspace {
            self.workspace = workspace;
            self.project_filter = ProjectFilter::All;
            self.reset_list_focus();
        }
    }

    pub fn mode(&self) -> ViewMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: ViewMode) {
        if self.mode != mode {
            self.mode = mode;
            self.reset_list_focus();
        }
    }

    pub fn project_filter(&self) -> &ProjectFilter {
        &self.project_filter
    }

    pub fn set_project_filter(&mut self, project_filter: ProjectFilter) {
        self.project_filter = project_filter;
        self.reset_list_focus();
    }

    pub fn focus(&self) -> FocusZone {
        self.focus
    }

    pub fn selected(&self) -> Option<PaneKey> {
        self.selected
    }

    fn reset_list_focus(&mut self) {
        self.focus = FocusZone::List;
        self.selected = None;
    }

    pub fn inbox_rows<'a>(
        &self,
        panes: &'a [PaneSummary],
        reads: &InboxReadState,
    ) -> InboxRows<'a> {
        reads.rows(self.project_filter.panes(self.workspace, panes))
    }

    pub fn kanban_panes<'a, 'state>(
        &'state self,
        panes: &'a [PaneSummary],
    ) -> impl Iterator<Item = &'a PaneSummary> + use<'a, 'state> {
        self.project_filter.panes(self.workspace, panes)
    }

    pub fn kanban_columns<'a>(&self, panes: &'a [PaneSummary]) -> KanbanColumns<'a> {
        let mut columns = std::array::from_fn(|_| Vec::new());
        for pane in self.kanban_panes(panes) {
            columns[pane.state.kanban_index()].push(pane);
        }
        for column in &mut columns {
            column.sort_by_key(|pane| {
                (
                    pane.since_ms.is_none(),
                    pane.since_ms,
                    pane.location.key.host.0,
                    pane.location.key.pane,
                )
            });
        }
        KanbanColumns(columns)
    }

    pub fn workspace_summary<'a>(
        &self,
        projects: &'a [Project],
        panes: &[PaneSummary],
        reads: &InboxReadState,
    ) -> WorkspaceSummary<'a> {
        let mut counts = HashMap::<&Option<GroupKey>, AttentionCount>::new();
        let mut all = AttentionCount::default();
        for (pane, kind) in self.workspace_attention(panes, reads) {
            all.add(kind);
            counts.entry(&pane.location.group).or_default().add(kind);
        }
        WorkspaceSummary {
            all,
            projects: projects
                .iter()
                .map(|project| ProjectSummary {
                    project,
                    needs_attention: counts.get(&project.group).copied().unwrap_or_default(),
                })
                .collect(),
        }
    }

    pub fn attention_count(&self, panes: &[PaneSummary], reads: &InboxReadState) -> AttentionCount {
        let mut count = AttentionCount::default();
        for (_, kind) in self.workspace_attention(panes, reads) {
            count.add(kind);
        }
        count
    }

    fn workspace_attention<'a, 'model>(
        &'model self,
        panes: &'a [PaneSummary],
        reads: &'model InboxReadState,
    ) -> impl Iterator<Item = (&'a PaneSummary, Attention)> + use<'a, 'model> {
        panes
            .iter()
            .filter(move |pane| pane.location.workspace == self.workspace)
            .filter_map(move |pane| reads.attention(pane).map(|event| (pane, event.kind)))
    }

    pub fn next_needing_attention(
        &mut self,
        panes: &[PaneSummary],
        reads: &mut InboxReadState,
    ) -> Option<PaneTarget> {
        let (next, _) = self
            .workspace_attention(panes, reads)
            .filter(|(pane, _)| Some(pane.location.key) != self.selected)
            .min_by_key(|(pane, kind)| (*kind, recency(pane)))?;
        if let Some(current) = panes.iter().find(|pane| {
            pane.location.workspace == self.workspace && Some(pane.location.key) == self.selected
        }) {
            reads.mark_read(current);
        }
        if !self.project_filter.contains(&next.location.group) {
            self.project_filter = ProjectFilter::All;
        }
        self.mode = ViewMode::Inbox;
        self.focus_list_item(next.location.key);
        Some(PaneTarget::from(&next.location))
    }
}

pub(super) struct KanbanColumns<'a>(pub [Vec<&'a PaneSummary>; 5]);

impl State {
    pub const KANBAN_ORDER: [Self; 5] = [
        Self::Waiting,
        Self::Working,
        Self::Done,
        Self::Idle,
        Self::Exited,
    ];

    fn kanban_index(self) -> usize {
        match self {
            Self::Waiting => 0,
            Self::Working => 1,
            Self::Done => 2,
            Self::Idle => 3,
            Self::Exited => 4,
        }
    }
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

    fn add(&mut self, kind: Attention) {
        match kind {
            Attention::Waiting => self.waiting += 1,
            Attention::Finished => self.finished += 1,
        }
    }
}

pub(super) struct WorkspaceSummary<'a> {
    pub all: AttentionCount,
    pub projects: Vec<ProjectSummary<'a>>,
}

pub(super) struct ProjectSummary<'a> {
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

impl<'a> InboxRows<'a> {
    fn visible(&self, show_other: bool) -> impl Iterator<Item = &'a PaneSummary> + '_ {
        self.attention
            .iter()
            .chain(
                self.other
                    .iter()
                    .take(if show_other { self.other.len() } else { 0 }),
            )
            .copied()
    }

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

    pub fn rows<'a>(&self, panes: impl IntoIterator<Item = &'a PaneSummary>) -> InboxRows<'a> {
        let mut attention = Vec::new();
        let mut other = Vec::new();
        for pane in panes {
            if let Some(event) = self.attention(pane) {
                attention.push((event.kind, pane));
            } else {
                other.push(pane);
            }
        }
        attention.sort_by_key(|(kind, pane)| (*kind, recency(pane)));
        other.sort_by_key(|pane| recency(pane));
        InboxRows {
            attention: attention.into_iter().map(|(_, pane)| pane).collect(),
            other,
        }
    }
}

fn recency(pane: &PaneSummary) -> (Reverse<Option<u64>>, u64, u64) {
    (
        Reverse(pane.since_ms),
        pane.location.key.host.0,
        pane.location.key.pane,
    )
}

#[cfg(test)]
mod tests;
