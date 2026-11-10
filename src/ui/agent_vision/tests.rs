use tty59_core::core::cli_agent::AgentDetectionSession;
use tty59_core::core::machine::AgentFacts;

use super::*;

fn location(workspace: WorkspaceId, host: HostId, pane: u64) -> PaneLocation {
    PaneLocation {
        key: PaneKey { host, pane },
        workspace,
        tab: TabId::new(),
        tab_title: format!("Task {pane}"),
    }
}

fn agent(
    workspace: WorkspaceId,
    pane: u64,
    status: AgentStatus,
    since_ms: Option<u64>,
    unseen_done: bool,
) -> PaneSummary {
    PaneSummary::from_live(
        location(workspace, HostId::LOCAL, pane),
        Some(CLIAgent::Codex),
        Some(&AgentSessionState {
            status,
            since_ms,
            ..AgentSessionState::default()
        }),
        unseen_done,
    )
    .unwrap()
}

fn ids(rows: &[&PaneSummary]) -> Vec<u64> {
    rows.iter().map(|row| row.location.key.pane).collect()
}

#[test]
fn inbox_orders_waiting_then_unseen_done_and_counts_each_project() {
    let first = Project {
        id: WorkspaceId::new(),
        name: "First".into(),
    };
    let second = Project {
        id: WorkspaceId::new(),
        name: "Second".into(),
    };
    let empty = Project {
        id: WorkspaceId::new(),
        name: "Empty".into(),
    };
    let projects = [first, second, empty];
    let [first, second, _] = projects.each_ref().map(|project| project.id);
    let panes = [
        agent(first, 1, AgentStatus::Done, Some(90), true),
        agent(first, 2, AgentStatus::Working, Some(80), true),
        agent(first, 3, AgentStatus::Waiting, Some(40), false),
        agent(second, 4, AgentStatus::Waiting, Some(20), false),
        agent(second, 5, AgentStatus::Done, Some(100), false),
        agent(second, 6, AgentStatus::Done, Some(50), true),
        agent(second, 7, AgentStatus::Waiting, None, false),
        agent(second, 8, AgentStatus::Idle, None, false),
    ];
    let mut reads = InboxReadState::default();
    let rows = reads.rows(&panes, None);
    assert_eq!(ids(&rows.attention), [3, 4, 7, 1, 6]);
    assert_eq!(ids(&rows.other), [5, 2, 8]);
    assert_eq!(ids(&reads.rows(&panes, Some(first)).attention), [3, 1]);

    let counts = reads.workspaces(&projects, &panes);
    assert_eq!(counts[0].project.name, "First");
    assert_eq!(
        counts[0].needs_attention,
        AttentionCount {
            waiting: 1,
            finished: 1
        }
    );
    assert_eq!(counts[1].needs_attention.total(), 3);
    assert_eq!(counts[2].needs_attention.total(), 0);
    reads.mark_read(&panes[0]);
    assert_eq!(
        reads.workspaces(&projects, &panes)[0]
            .needs_attention
            .total(),
        1
    );
}

#[test]
fn handling_a_message_advances_without_changing_other_hosts_or_losing_selection() {
    let project = WorkspaceId::new();
    let other_project = WorkspaceId::new();
    let mut other_host = agent(other_project, 1, AgentStatus::Waiting, Some(20), false);
    other_host.location.key.host = HostId(9);
    let current = agent(project, 1, AgentStatus::Waiting, Some(30), false);
    let current_key = current.location.key;
    let other_key = other_host.location.key;
    let panes = [other_host, current];
    let mut reads = InboxReadState::default();

    assert_eq!(
        reads.handle_and_advance(&panes, Some(current_key)),
        Some(other_key)
    );
    let rows = reads.rows(&panes, None);
    assert_eq!(rows.attention[0].location.key, other_key);
    assert_eq!(rows.other[0].location.key, current_key);
    assert_eq!(rows.keep_selection(Some(current_key)), Some(current_key));
    assert_eq!(
        reads
            .rows(&panes[..1], None)
            .keep_selection(Some(current_key)),
        Some(other_key)
    );
    assert_eq!(reads.handle_and_advance(&panes, Some(other_key)), None);
}

#[test]
fn read_marks_follow_a_pane_and_expire_for_a_new_event() {
    let workspace = WorkspaceId::new();
    let mut pane = agent(workspace, 1, AgentStatus::Waiting, None, false);
    let mut reads = InboxReadState::default();
    reads.mark_read(&pane);
    pane.location.workspace = WorkspaceId::new();
    pane.location.tab = TabId::new();
    reads.observe(&[]);
    assert!(
        reads
            .rows(std::slice::from_ref(&pane), None)
            .attention
            .is_empty()
    );

    pane.state = State::Working;
    reads.observe(std::slice::from_ref(&pane));
    assert!(
        reads
            .rows(std::slice::from_ref(&pane), None)
            .attention
            .is_empty()
    );
    pane.state = State::Waiting;
    assert_eq!(
        ids(&reads.rows(std::slice::from_ref(&pane), None).attention),
        [1]
    );

    pane.state = State::Done;
    pane.unseen_done = true;
    pane.turns = 1;
    reads.mark_read(&pane);
    assert!(
        reads
            .rows(std::slice::from_ref(&pane), None)
            .attention
            .is_empty()
    );
    pane.turns = 2;
    assert_eq!(
        ids(&reads.rows(std::slice::from_ref(&pane), None).attention),
        [1]
    );
}

#[test]
fn an_already_seen_result_stays_read_when_only_the_mirror_remains() {
    let mut pane = agent(WorkspaceId::new(), 1, AgentStatus::Done, Some(20), false);
    pane.epoch = Some(3);
    let mut reads = InboxReadState::default();
    reads.observe(std::slice::from_ref(&pane));
    pane.unseen_done = true;
    assert!(
        reads
            .rows(std::slice::from_ref(&pane), None)
            .attention
            .is_empty()
    );
    pane.epoch = Some(4);
    assert_eq!(
        ids(&reads.rows(std::slice::from_ref(&pane), None).attention),
        [1]
    );
}

#[test]
fn pane_summaries_use_live_or_mirrored_agent_identity_not_a_shell() {
    let location = location(WorkspaceId::new(), HostId(8), 1);
    let mut record = PaneRecord::new(1);
    record.live = true;
    assert!(PaneSummary::from_live(location.clone(), None, None, false).is_none());
    assert!(PaneSummary::from_mirror(location.clone(), &record, false).is_none());
    record.agent = Some(AgentFacts {
        agent: CLIAgent::Claude,
        session_id: None,
        launch_argv: None,
        status: Some(AgentStatus::Waiting),
    });
    let mirrored = PaneSummary::from_mirror(location.clone(), &record, false).unwrap();
    assert_eq!(mirrored.agent_name(), CLIAgent::Claude.display_name());
    assert_eq!(mirrored.state, State::Waiting);

    let state = AgentSessionState {
        status: AgentStatus::Working,
        since_ms: Some(25),
        detection: Some(AgentDetectionSession {
            kind: "devin".into(),
            epoch: 3,
            exited: false,
            source: None,
            rule: None,
        }),
        ..AgentSessionState::default()
    };
    record.agent = None;
    record.agent_overview = state.overview();
    let live = PaneSummary::from_live(location.clone(), None, Some(&state), false).unwrap();
    assert_eq!(
        live,
        PaneSummary::from_mirror(location.clone(), &record, false).unwrap()
    );
    assert_eq!(live.agent_name(), "devin");

    record.agent_overview.as_mut().unwrap().exited = true;
    let exited = PaneSummary::from_mirror(location.clone(), &record, false).unwrap();
    assert_eq!(exited.state, State::Exited);
    let rows = InboxReadState::default().rows(std::slice::from_ref(&exited), None);
    assert!(rows.attention.is_empty() && rows.other.is_empty());
    record.live = false;
    assert!(PaneSummary::from_mirror(location, &record, false).is_none());
}
