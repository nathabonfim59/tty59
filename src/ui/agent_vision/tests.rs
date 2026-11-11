use tty59_core::core::cli_agent::AgentDetectionSession;
use tty59_core::core::group_key::{AutoKey, GroupId};
use tty59_core::core::machine::AgentFacts;

use super::*;

fn location(workspace: WorkspaceId, host: HostId, pane: u64) -> PaneLocation {
    PaneLocation {
        key: PaneKey { host, pane },
        workspace,
        tab: TabId::new(),
        tab_title: format!("Task {pane}"),
        group: None,
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
fn agent_search_matches_title_kind_project_and_reason_together() {
    let workspace = WorkspaceId::new();
    let mut pane = agent(workspace, 1, AgentStatus::Waiting, Some(30), false);
    pane.location.tab_title = "Review authentication".into();
    pane.reason = Some("Approve the test command".into());
    assert!(pane.matches_search("authentication codex payments approve", "Payments"));
    assert!(!pane.matches_search("authentication another-project", "Payments"));
    assert!(pane.matches_search("", "Payments"));
}

fn repo_group(path: &str) -> Option<GroupKey> {
    Some(GroupKey::Auto(AutoKey::Repo(path.into())))
}

#[test]
fn inbox_orders_waiting_then_unseen_done_and_counts_each_project() {
    let workspace = WorkspaceId::new();
    let other_workspace = WorkspaceId::new();
    let first = Project {
        group: repo_group("/work/first"),
        name: "First".into(),
    };
    let second = Project {
        group: Some(GroupKey::Pinned(GroupId::new())),
        name: "Second".into(),
    };
    let empty = Project {
        group: repo_group("/work/empty"),
        name: "Empty".into(),
    };
    let projects = [first, second, empty];
    let mut panes = [
        agent(workspace, 1, AgentStatus::Done, Some(90), true),
        agent(workspace, 2, AgentStatus::Working, Some(80), true),
        agent(workspace, 3, AgentStatus::Waiting, Some(40), false),
        agent(workspace, 4, AgentStatus::Waiting, Some(20), false),
        agent(workspace, 5, AgentStatus::Done, Some(100), false),
        agent(workspace, 6, AgentStatus::Done, Some(50), true),
        agent(workspace, 7, AgentStatus::Waiting, None, false),
        agent(workspace, 8, AgentStatus::Idle, None, false),
        agent(other_workspace, 9, AgentStatus::Waiting, Some(200), false),
    ];
    for pane in &mut panes[..3] {
        pane.location.group.clone_from(&projects[0].group);
    }
    for pane in &mut panes[3..8] {
        pane.location.group.clone_from(&projects[1].group);
    }
    panes[8].location.group.clone_from(&projects[0].group);
    let mut vision = VisionState::new(workspace);
    let mut reads = InboxReadState::default();
    let rows = vision.inbox_rows(&panes, &reads);
    assert_eq!(ids(&rows.attention), [3, 4, 7, 1, 6]);
    assert_eq!(ids(&rows.other), [5, 2, 8]);
    vision.set_project_filter(ProjectFilter::Group(projects[0].group.clone()));
    assert_eq!(ids(&vision.inbox_rows(&panes, &reads).attention), [3, 1]);

    let counts = vision.workspace_summary(&projects, &panes, &reads);
    assert_eq!(counts.projects[0].project.name, "First");
    assert_eq!(
        counts.projects[0].needs_attention,
        AttentionCount {
            waiting: 1,
            finished: 1
        }
    );
    assert_eq!(counts.projects[1].needs_attention.total(), 3);
    assert_eq!(counts.projects[2].needs_attention.total(), 0);
    assert_eq!(counts.all.total(), 5);
    assert_eq!(vision.attention_count(&panes, &reads), counts.all);
    reads.mark_read(&panes[0]);
    assert_eq!(
        vision.workspace_summary(&projects, &panes, &reads).projects[0]
            .needs_attention
            .total(),
        1
    );
    vision.focus_list_item(panes[2].location.key);
    let next = vision.next_needing_attention(&panes, &mut reads);
    assert_eq!(next, Some(PaneTarget::from(&panes[3].location)));
    assert_eq!(vision.project_filter(), &ProjectFilter::All);
}

#[test]
fn handling_a_message_advances_without_changing_other_hosts_or_losing_selection() {
    let project = WorkspaceId::new();
    let mut other_host = agent(project, 1, AgentStatus::Waiting, Some(20), false);
    other_host.location.key.host = HostId(9);
    let current = agent(project, 1, AgentStatus::Waiting, Some(30), false);
    let current_key = current.location.key;
    let other_key = other_host.location.key;
    let other_target = PaneTarget::from(&other_host.location);
    let panes = [other_host, current];
    let mut reads = InboxReadState::default();
    let mut vision = VisionState::new(project);
    vision.focus_list_item(current_key);

    assert_eq!(
        vision.next_needing_attention(&panes, &mut reads),
        Some(other_target)
    );
    let rows = reads.rows(&panes);
    assert_eq!(rows.attention[0].location.key, other_key);
    assert_eq!(rows.other[0].location.key, current_key);
    assert_eq!(rows.keep_selection(Some(current_key)), Some(current_key));
    assert_eq!(
        reads.rows(&panes[..1]).keep_selection(Some(current_key)),
        Some(other_key)
    );
    assert_eq!(vision.next_needing_attention(&panes, &mut reads), None);
    assert_eq!(vision.selected(), Some(other_key));
    assert_eq!(reads.rows(&panes).attention[0].location.key, other_key);
}

#[test]
fn next_attention_does_not_leave_the_workspace_when_no_other_agent_is_waiting() {
    let workspace = WorkspaceId::new();
    let mut current = agent(workspace, 1, AgentStatus::Waiting, Some(30), false);
    let group = repo_group("/work/current");
    current.location.group.clone_from(&group);
    let panes = [
        current,
        agent(WorkspaceId::new(), 2, AgentStatus::Waiting, Some(50), false),
    ];
    let mut vision = VisionState::new(workspace);
    vision.set_project_filter(ProjectFilter::Group(group.clone()));
    let mut reads = InboxReadState::default();
    vision.open_inbox_terminal(&panes[0], &reads);
    assert_eq!(vision.next_needing_attention(&panes, &mut reads), None);
    assert_eq!(vision.focus(), FocusZone::Terminal);
    assert_eq!(vision.selected(), Some(panes[0].location.key));
    assert_eq!(vision.project_filter(), &ProjectFilter::Group(group));
    assert_eq!(vision.attention_count(&panes, &reads).total(), 1);
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
    assert!(reads.rows(std::slice::from_ref(&pane)).attention.is_empty());

    pane.state = State::Working;
    reads.observe(std::slice::from_ref(&pane));
    assert!(reads.rows(std::slice::from_ref(&pane)).attention.is_empty());
    pane.state = State::Waiting;
    assert_eq!(ids(&reads.rows(std::slice::from_ref(&pane)).attention), [1]);

    pane.state = State::Done;
    pane.unseen_done = true;
    pane.turns = 1;
    reads.mark_read(&pane);
    assert!(reads.rows(std::slice::from_ref(&pane)).attention.is_empty());
    pane.turns = 2;
    assert_eq!(ids(&reads.rows(std::slice::from_ref(&pane)).attention), [1]);
}

#[test]
fn an_already_seen_result_stays_read_when_only_the_mirror_remains() {
    let mut pane = agent(WorkspaceId::new(), 1, AgentStatus::Done, Some(20), false);
    pane.epoch = Some(3);
    let mut reads = InboxReadState::default();
    reads.observe(std::slice::from_ref(&pane));
    pane.unseen_done = true;
    assert!(reads.rows(std::slice::from_ref(&pane)).attention.is_empty());
    pane.epoch = Some(4);
    assert_eq!(ids(&reads.rows(std::slice::from_ref(&pane)).attention), [1]);
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
    let rows = InboxReadState::default().rows(std::slice::from_ref(&exited));
    assert!(rows.attention.is_empty());
    assert_eq!(ids(&rows.other), [1]);
    record.live = false;
    assert!(PaneSummary::from_mirror(location, &record, false).is_none());
}

#[test]
fn project_filter_survives_switching_between_inbox_and_kanban() {
    let workspace = WorkspaceId::new();
    let other_workspace = WorkspaceId::new();
    let first = repo_group("/work/first");
    let second = repo_group("/work/second");
    let mut exited = agent(workspace, 3, AgentStatus::Idle, Some(20), false);
    exited.state = State::Exited;
    let mut panes = [
        agent(workspace, 1, AgentStatus::Waiting, Some(10), false),
        agent(workspace, 2, AgentStatus::Waiting, Some(30), false),
        exited,
        agent(other_workspace, 4, AgentStatus::Waiting, Some(40), false),
    ];
    panes[0].location.group.clone_from(&first);
    for pane in &mut panes[1..] {
        pane.location.group.clone_from(&second);
    }
    let reads = InboxReadState::default();
    let mut vision = VisionState::new(workspace);
    vision.set_mode(ViewMode::Inbox);
    vision.set_project_filter(ProjectFilter::Group(second));
    assert_eq!(ids(&vision.inbox_rows(&panes, &reads).attention), [2]);
    vision.set_mode(ViewMode::Kanban);
    assert_eq!(
        ids(&vision.kanban_panes(&panes).collect::<Vec<_>>()),
        [2, 3]
    );
    vision.set_project_filter(ProjectFilter::Group(first));
    vision.set_mode(ViewMode::Inbox);
    assert_eq!(ids(&vision.inbox_rows(&panes, &reads).attention), [1]);
    vision.set_workspace(workspace);
    assert_eq!(ids(&vision.inbox_rows(&panes, &reads).attention), [1]);
    vision.set_workspace(other_workspace);
    assert_eq!(vision.project_filter(), &ProjectFilter::All);
    assert_eq!(ids(&vision.inbox_rows(&panes, &reads).attention), [4]);
}

#[test]
fn project_filter_cycles_through_all_and_recovers_when_a_project_disappears() {
    let first = repo_group("/work/first");
    let second = None;
    let projects = [
        Project {
            group: first.clone(),
            name: "First".into(),
        },
        Project {
            group: second.clone(),
            name: "Second".into(),
        },
    ];
    let filter = ProjectFilter::All;
    assert_eq!(
        filter.cycle(&projects, true),
        ProjectFilter::Group(first.clone())
    );
    assert_eq!(
        filter.cycle(&projects, false),
        ProjectFilter::Group(second.clone())
    );
    let mut filter = ProjectFilter::Group(second);
    assert_eq!(filter.cycle(&projects, true), ProjectFilter::All);
    assert_eq!(filter.cycle(&projects, false), ProjectFilter::Group(first));
    filter.reconcile(&projects[..1]);
    assert_eq!(filter, ProjectFilter::All);
    assert_eq!(filter.cycle(&[], false), ProjectFilter::All);
}

#[test]
fn inbox_navigation_stays_in_the_list_until_enter_and_leaves_terminal_keys_alone() {
    let workspace = WorkspaceId::new();
    let panes = [
        agent(workspace, 1, AgentStatus::Waiting, Some(30), false),
        agent(workspace, 2, AgentStatus::Waiting, Some(20), false),
    ];
    let reads = InboxReadState::default();
    let mut vision = VisionState::new(workspace);
    vision.set_mode(ViewMode::Inbox);
    vision.reconcile_focus(&panes, &reads);
    assert_eq!(vision.focus(), FocusZone::List);
    assert_eq!(vision.selected(), Some(panes[0].location.key));
    for key in ["j", "up", "down"] {
        assert_eq!(
            vision.navigate(NavigationKey::from_key(key).unwrap(), &panes, &reads, &[]),
            NavigationResult::List
        );
    }
    assert_eq!(vision.selected(), Some(panes[1].location.key));
    assert_eq!(
        vision.navigate(NavigationKey::Enter, &panes, &reads, &[]),
        NavigationResult::Terminal(PaneTarget::from(&panes[1].location))
    );
    assert_eq!(vision.focus(), FocusZone::Terminal);
    for key in [
        "[", "]", "h", "j", "k", "l", "up", "down", "left", "right", "enter",
    ] {
        assert_eq!(
            vision.navigate(NavigationKey::from_key(key).unwrap(), &panes, &reads, &[]),
            NavigationResult::Unhandled,
            "{key}"
        );
    }
    assert_eq!(
        vision.navigate(NavigationKey::Escape, &panes, &reads, &[]),
        NavigationResult::List
    );
    assert_eq!(vision.focus(), FocusZone::List);
    assert_eq!(vision.selected(), Some(panes[1].location.key));
}

#[test]
fn changing_project_or_workspace_resets_focus_to_the_first_item_in_the_list() {
    let workspace = WorkspaceId::new();
    let other_workspace = WorkspaceId::new();
    let project = Project {
        group: repo_group("/work/second"),
        name: "Second".into(),
    };
    let mut panes = [
        agent(workspace, 1, AgentStatus::Waiting, Some(30), false),
        agent(workspace, 2, AgentStatus::Waiting, Some(20), false),
        agent(other_workspace, 3, AgentStatus::Waiting, Some(50), false),
    ];
    panes[1].location.group.clone_from(&project.group);
    let reads = InboxReadState::default();
    let mut vision = VisionState::new(workspace);
    vision.set_mode(ViewMode::Inbox);
    vision.open_inbox_terminal(&panes[0], &reads);
    vision.set_project_filter(ProjectFilter::Group(project.group.clone()));
    vision.reconcile_focus(&panes, &reads);
    assert_eq!(vision.focus(), FocusZone::List);
    assert_eq!(vision.selected(), Some(panes[1].location.key));
    assert_eq!(
        vision.navigate(NavigationKey::PreviousProject, &panes, &reads, &[project]),
        NavigationResult::List
    );
    assert_eq!(vision.selected(), Some(panes[0].location.key));

    vision.open_inbox_terminal(&panes[0], &reads);
    vision.set_workspace(other_workspace);
    vision.reconcile_focus(&panes, &reads);
    assert_eq!(vision.focus(), FocusZone::List);
    assert_eq!(vision.selected(), Some(panes[2].location.key));
}

#[test]
fn kanban_navigation_skips_empty_columns_clamps_rows_and_opens_inbox_terminal() {
    let workspace = WorkspaceId::new();
    let panes = [
        agent(workspace, 1, AgentStatus::Working, Some(10), false),
        agent(workspace, 2, AgentStatus::Working, Some(20), false),
        agent(workspace, 3, AgentStatus::Working, Some(30), false),
        agent(workspace, 4, AgentStatus::Idle, Some(40), false),
    ];
    let reads = InboxReadState::default();
    let mut vision = VisionState::new(workspace);
    vision.set_mode(ViewMode::Kanban);
    vision.reconcile_focus(&panes, &reads);
    assert_eq!(vision.selected(), Some(panes[0].location.key));
    for key in ["j", "down", "down"] {
        vision.navigate(NavigationKey::from_key(key).unwrap(), &panes, &reads, &[]);
    }
    assert_eq!(vision.selected(), Some(panes[2].location.key));
    vision.navigate(NavigationKey::Right, &panes, &reads, &[]);
    assert_eq!(vision.selected(), Some(panes[3].location.key));
    vision.navigate(NavigationKey::Left, &panes, &reads, &[]);
    assert_eq!(vision.selected(), Some(panes[0].location.key));
    vision.navigate(NavigationKey::Right, &panes, &reads, &[]);
    assert_eq!(
        vision.navigate(NavigationKey::Enter, &panes, &reads, &[]),
        NavigationResult::Terminal(PaneTarget::from(&panes[3].location))
    );
    assert_eq!(vision.mode(), ViewMode::Inbox);
    assert_eq!(vision.focus(), FocusZone::Terminal);
    assert!(vision.show_other);
    vision.navigate(NavigationKey::Escape, &panes, &reads, &[]);
    assert_eq!(vision.selected(), Some(panes[3].location.key));
    assert_eq!(vision.focus(), FocusZone::List);
}
