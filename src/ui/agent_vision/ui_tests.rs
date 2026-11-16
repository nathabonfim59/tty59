use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};

use super::render::{RenderedHelp, take_rendered_headers, take_rendered_help, take_rendered_rows};
use super::{FocusZone, State, ViewMode};
use crate::core::cli_agent::{AgentDetectionSession, AgentSessionState, AgentStatus, CLIAgent};
use crate::daemon::protocol::DaemonMsg;
use crate::ui::app::Tty59App;

fn agents(
    cx: &mut TestAppContext,
) -> (
    Entity<Tty59App>,
    VisualTestContext,
    Vec<crate::daemon::transport::Stream>,
) {
    agents_in(cx, AgentStatus::Waiting)
}

/// Two agents that are all in `status`: Waiting ones need attention and fill
/// the Inbox's top group, Working ones sit in Other.
fn agents_in(
    cx: &mut TestAppContext,
    status: AgentStatus,
) -> (
    Entity<Tty59App>,
    VisualTestContext,
    Vec<crate::daemon::transport::Stream>,
) {
    let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 3);
    for (index, stream) in streams.iter_mut().take(2).enumerate() {
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status,
            since_ms: Some(index as u64 + 1),
            message: Some(format!("Approve task {index}")),
            detection: Some(AgentDetectionSession {
                kind: "codex".into(),
                epoch: 1,
                exited: false,
                source: None,
                rule: None,
            }),
            ..Default::default()
        }))
        .encode(stream)
        .unwrap();
    }
    for _ in 0..200 {
        if app.update_in(&mut vcx, |app, window, cx| {
            app.refresh_agent_vision(window, cx);
            app.agent_vision.panes.len()
        }) == 2
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.agent_vision.panes.len(), 2);
        app.set_agent_mode(ViewMode::Inbox, window, cx);
    });
    vcx.run_until_parked();
    (app, vcx, streams)
}

#[gpui::test]
fn multiline_agent_icons_align_with_titles_and_keep_card_padding(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    for (mode, inset) in [(ViewMode::Inbox, 8.), (ViewMode::Kanban, 9.)] {
        app.update_in(&mut vcx, |app, window, cx| {
            app.set_agent_mode(mode, window, cx);
        });
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let row = vcx
            .debug_bounds("agent-row-frame-0-1")
            .expect("agent row was painted");
        let icon = vcx
            .debug_bounds("agent-row-icon-0-1")
            .expect("agent icon was painted");
        let title = vcx
            .debug_bounds("agent-row-title-0-1")
            .expect("agent title was painted");
        assert!(
            row.size.height > icon.size.height + gpui::px(16.),
            "fixture has multiple lines"
        );
        assert!(
            (icon.top() - title.top()).abs() <= gpui::px(0.5),
            "agent icon must align with title in {mode:?}: icon {icon:?}, title {title:?}"
        );
        assert_eq!(icon.size.height, gpui::px(22.));
        assert_eq!(icon.top() - row.top(), gpui::px(inset));
        assert_eq!(icon.left() - row.left(), gpui::px(inset));
    }
}

#[gpui::test]
fn list_enter_focuses_the_existing_terminal_and_escape_returns_to_list(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    let first = app.update_in(&mut vcx, |app, _, _| {
        app.agent_vision.state.selected().unwrap()
    });
    vcx.simulate_keystrokes("j");
    let second = app.update_in(&mut vcx, |app, _, _| {
        app.agent_vision.state.selected().unwrap()
    });
    assert_ne!(first, second);
    vcx.simulate_keystrokes("enter");
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.agent_vision.state.focus(), FocusZone::Terminal);
        let terminal = app.agent_terminal(second, cx).unwrap();
        assert!(terminal.focus_handle(cx).contains_focused(window, cx));
        assert_eq!(app.tabs.len(), 3, "reader reuses the existing pane");
    });
    vcx.simulate_keystrokes("j");
    app.update_in(&mut vcx, |app, _, _| {
        assert_eq!(app.agent_vision.state.selected(), Some(second))
    });
    vcx.simulate_keystrokes("escape");
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.agent_vision.state.focus(), FocusZone::List);
        assert!(app.agent_vision.list_focus.is_focused(window));
        assert_eq!(app.agent_vision.state.selected(), Some(second));
        assert_eq!(
            app.agent_vision
                .state
                .attention_count(&app.agent_vision.panes, &app.agent_vision.reads)
                .total(),
            2
        );
        app.next_agent_attention(window, cx);
        assert_eq!(app.agent_vision.state.selected(), Some(first));
        assert_eq!(app.agent_vision.state.focus(), FocusZone::Terminal);
        assert_eq!(
            app.agent_vision
                .state
                .attention_count(&app.agent_vision.panes, &app.agent_vision.reads)
                .total(),
            1
        );
    });
}

#[gpui::test]
fn search_owns_plain_keys_and_enter_returns_to_the_filtered_list(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        window.focus(&app.agent_vision.input.focus_handle(cx), cx)
    });
    vcx.simulate_keystrokes("j");
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.agent_vision.input.read(cx).value().as_str(), "j");
        assert!(
            app.agent_vision
                .input
                .focus_handle(cx)
                .contains_focused(window, cx)
        );
    });
    vcx.simulate_keystrokes("escape");
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.agent_vision.input.read(cx).value().is_empty());
        assert!(app.agent_vision.list_focus.is_focused(window));
        let input = app.agent_vision.input.clone();
        input.update(cx, |input, cx| input.set_value("task 0", window, cx));
        window.focus(&input.focus_handle(cx), cx);
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.agent_vision.filtered(cx).len(), 1);
        assert_eq!(app.agent_vision.state.focus(), FocusZone::List);
        assert!(app.agent_vision.list_focus.is_focused(window));
        app.set_agent_mode(ViewMode::Kanban, window, cx);
        assert_eq!(app.agent_vision.filtered(cx).len(), 1);
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.agent_vision.state.mode(), ViewMode::Inbox);
        assert_eq!(app.agent_vision.state.focus(), FocusZone::Terminal);
        let selected = app.agent_vision.state.selected().unwrap();
        assert!(
            app.agent_terminal(selected, cx)
                .unwrap()
                .focus_handle(cx)
                .contains_focused(window, cx)
        );
    });
}

#[test]
fn agent_mode_setting_preserves_existing_users_and_round_trips() {
    let old: tty59_core::core::config::Config = serde_json::from_str("{}").unwrap();
    assert!(!old.agent_vision_default);
    let configured: tty59_core::core::config::Config =
        serde_json::from_str(r#"{"agent_vision_default":true}"#).unwrap();
    let roundtrip: tty59_core::core::config::Config =
        serde_json::from_value(serde_json::to_value(configured).unwrap()).unwrap();
    assert!(roundtrip.agent_vision_default);
}

#[gpui::test]
fn preview_keeps_short_output_near_the_top_of_a_tall_grid(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = agents(cx);
    DaemonMsg::Output(b"Prepared the authentication tests\r\n".to_vec())
        .encode(&mut streams[0])
        .unwrap();
    let mut preview = None;
    for _ in 0..200 {
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        vcx.run_until_parked();
        preview = app.update_in(&mut vcx, |app, _, cx| {
            app.agent_pane_summaries(cx)
                .into_iter()
                .find(|pane| pane.location.key.pane == 1)
                .and_then(|pane| pane.last_line)
        });
        if preview.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        preview.as_deref(),
        Some("Prepared the authentication tests")
    );
}

#[gpui::test]
fn changing_workspace_returns_agent_mode_to_the_list(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        let key = app.agent_vision.state.selected().unwrap();
        app.open_agent_terminal(key, window, cx);
        assert_eq!(app.agent_vision.state.focus(), FocusZone::Terminal);
        app.workspace = crate::core::session::WorkspaceId::new();
        app.refresh_agent_vision(window, cx);
        assert_eq!(app.agent_vision.state.focus(), FocusZone::List);
        assert!(app.agent_vision.list_focus.is_focused(window));
        assert_eq!(
            app.agent_vision.state.project_filter(),
            &super::ProjectFilter::All
        );
        assert!(app.agent_vision.input.read(cx).value().is_empty());
    });
}

#[gpui::test]
fn renaming_a_workspace_does_not_handle_the_agent_in_the_reader(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    vcx.simulate_keystrokes("enter");
    app.update_in(&mut vcx, |app, window, cx| {
        app.start_workspace_rename(window, cx)
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("r e n a m e d enter");
    app.update_in(&mut vcx, |app, window, _| {
        assert_eq!(
            app.agent_vision
                .state
                .attention_count(&app.agent_vision.panes, &app.agent_vision.reads)
                .total(),
            2,
            "Enter in the workspace name field is not an answer to the agent"
        );
        assert!(app.agent_vision.list_focus.is_focused(window));
    });
}

#[gpui::test]
fn opening_a_background_agent_activates_its_tab_for_existing_pane_actions(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    let selected = app.update_in(&mut vcx, |app, _, _| {
        app.agent_vision.state.selected().unwrap()
    });
    assert_eq!(selected.pane, 2, "the newest wait is in the background tab");
    vcx.simulate_keystrokes("enter");
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(
            app.tabs[app.active]
                .pane
                .terminals()
                .iter()
                .any(|terminal| {
                    let view = terminal.read(cx);
                    view.host_id() == selected.host && view.pane_id == selected.pane
                }),
            "pane actions must use the same tab as the live reader"
        );
        assert!(
            app.agent_terminal(selected, cx)
                .unwrap()
                .focus_handle(cx)
                .contains_focused(window, cx)
        );
        assert_eq!(app.tabs.len(), 3);
    });
}

#[gpui::test]
fn exited_agent_preserves_identity_across_collector_inbox_and_kanban(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 4);
    let now_s = crate::core::config::unix_now();
    for (stream, (dir, agent, kind, status)) in streams.iter_mut().zip([
        (
            "/tmp/billing-service",
            CLIAgent::Codex,
            "codex",
            AgentStatus::Working,
        ),
        (
            "/tmp/mobile-app",
            CLIAgent::Gemini,
            "gemini",
            AgentStatus::Working,
        ),
        (
            "/tmp/acme-web",
            CLIAgent::Claude,
            "claude",
            AgentStatus::Working,
        ),
        ("/tmp/idle-app", CLIAgent::Codex, "codex", AgentStatus::Idle),
    ]) {
        DaemonMsg::Cwd(std::path::PathBuf::from(dir))
            .encode(stream)
            .unwrap();
        DaemonMsg::Agent(Some(agent)).encode(stream).unwrap();
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status,
            since_ms: Some(now_s.saturating_sub(60) * 1000),
            detection: Some(AgentDetectionSession {
                kind: kind.into(),
                epoch: 1,
                exited: false,
                source: None,
                rule: None,
            }),
            ..Default::default()
        }))
        .encode(stream)
        .unwrap();
    }
    DaemonMsg::Output(b"Running auth tests\r\n".to_vec())
        .encode(&mut streams[2])
        .unwrap();

    let mut live_ready = false;
    for _ in 0..200 {
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        vcx.run_until_parked();
        live_ready = app.read_with(&vcx, |app, cx| {
            let summaries = app.agent_pane_summaries(cx);
            summaries.len() == 4
                && summaries
                    .iter()
                    .find(|pane| pane.location.key.pane == 3)
                    .and_then(|pane| pane.last_line.as_deref())
                    == Some("Running auth tests")
        });
        if live_ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(live_ready, "initial live agent states and preview arrived");

    // Transition panes 1 and 2 to exited via real daemon reports; pane 1 emits a shell prompt.
    DaemonMsg::Output(b"\x1b[2J\x1b[H[main]:~/demo/billing-service$ \r\n".to_vec())
        .encode(&mut streams[0])
        .unwrap();
    DaemonMsg::AgentStatus(Some(AgentSessionState {
        status: AgentStatus::Idle,
        since_ms: Some(now_s.saturating_sub(180) * 1000),
        detection: Some(AgentDetectionSession {
            kind: "codex".into(),
            epoch: 2,
            exited: true,
            source: None,
            rule: None,
        }),
        ..Default::default()
    }))
    .encode(&mut streams[0])
    .unwrap();
    DaemonMsg::Agent(None).encode(&mut streams[0]).unwrap();

    DaemonMsg::AgentStatus(Some(AgentSessionState {
        status: AgentStatus::Idle,
        since_ms: None,
        detection: Some(AgentDetectionSession {
            kind: "gemini".into(),
            epoch: 2,
            exited: true,
            source: None,
            rule: None,
        }),
        ..Default::default()
    }))
    .encode(&mut streams[1])
    .unwrap();
    DaemonMsg::Agent(None).encode(&mut streams[1]).unwrap();

    let mut exit_ready = false;
    for _ in 0..200 {
        vcx.run_until_parked();
        exit_ready = app.read_with(&vcx, |app, cx| {
            let terminal_has_prompt = app.tabs[0].pane.first_leaf().is_some_and(|slot| {
                slot.terminal()
                    .is_some_and(|term| term.read(cx).agent_preview().is_some())
            });
            let summaries = app.agent_pane_summaries(cx);
            let p1 = summaries.iter().find(|p| p.location.key.pane == 1);
            let p2 = summaries.iter().find(|p| p.location.key.pane == 2);
            terminal_has_prompt
                && p1.is_some_and(|p| p.state == State::Exited)
                && p2.is_some_and(|p| p.state == State::Exited)
        });
        if exit_ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(exit_ready, "exited daemon state and shell prompt settled");

    app.read_with(&vcx, |app, cx| {
        let summaries = app.agent_pane_summaries(cx);
        let p1 = summaries
            .iter()
            .find(|p| p.location.key.pane == 1)
            .expect("exited codex pane remains in collector");
        assert_eq!(p1.agent, Some(CLIAgent::Codex));
        assert_eq!(p1.agent_name(), CLIAgent::Codex.display_name());
        assert_eq!(
            p1.last_line, None,
            "exited agent suppresses shell prompt preview"
        );

        let p2 = summaries
            .iter()
            .find(|p| p.location.key.pane == 2)
            .expect("exited gemini pane remains in collector");
        assert_eq!(p2.agent, Some(CLIAgent::Gemini));
        assert_eq!(p2.since_ms, None);
        assert_eq!(p2.last_line, None);

        let p3 = summaries
            .iter()
            .find(|p| p.location.key.pane == 3)
            .expect("live claude pane remains in collector");
        assert_eq!(p3.state, State::Working);
        assert_eq!(p3.last_line.as_deref(), Some("Running auth tests"));
    });

    // Render Inbox with Other agents expanded and the exited Codex pane selected.
    app.update_in(&mut vcx, |app, window, cx| {
        app.set_agent_mode(ViewMode::Inbox, window, cx);
        app.agent_vision.state.show_other = true;
        let key1 = app
            .agent_vision
            .panes
            .iter()
            .find(|p| p.location.key.pane == 1)
            .unwrap()
            .location
            .key;
        app.agent_vision.state.focus_list_item(key1);
        cx.notify();
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let _ = take_rendered_rows();
    let _ = take_rendered_headers();
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    let inbox_rows = take_rendered_rows();
    let inbox_headers = take_rendered_headers();

    let inbox_p1 = inbox_rows
        .iter()
        .find(|row| !row.card && row.key.pane == 1)
        .expect("exited codex row rendered in Inbox");
    assert_eq!(inbox_p1.title, CLIAgent::Codex.display_name());
    assert_eq!(inbox_p1.reason, "Exited · 3m");

    let reader_header = inbox_headers
        .iter()
        .find(|header| header.key.pane == 1)
        .expect("exited reader header rendered");
    assert_eq!(reader_header.title, CLIAgent::Codex.display_name());

    let inbox_p2 = inbox_rows
        .iter()
        .find(|row| !row.card && row.key.pane == 2)
        .expect("exited gemini row rendered in Inbox");
    assert_eq!(inbox_p2.title, CLIAgent::Gemini.display_name());
    assert_eq!(inbox_p2.reason, "Exited");

    let inbox_p3 = inbox_rows
        .iter()
        .find(|row| !row.card && row.key.pane == 3)
        .expect("live claude row rendered in Inbox");
    assert_eq!(inbox_p3.title, CLIAgent::Claude.display_name());
    // A5.1: a bare Working is not repeated in the Other group, which already
    // says it; the row built no reason line.
    assert_eq!(inbox_p3.reason, "");
    assert!(vcx.debug_bounds("agent-row-reason-0-3").is_none());
    assert!(vcx.debug_bounds("row-status-circle-0").is_some());

    // Render Kanban and verify cards, absent preview, and absent dot.
    app.update_in(&mut vcx, |app, window, cx| {
        app.set_agent_mode(ViewMode::Kanban, window, cx);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let _ = take_rendered_rows();
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    let kanban_rows = take_rendered_rows();
    let card_p1 = kanban_rows
        .iter()
        .find(|row| row.card && row.key.pane == 1)
        .expect("exited codex card rendered in Kanban");
    assert_eq!(card_p1.title, CLIAgent::Codex.display_name());
    assert_eq!(card_p1.reason, "Exited · 3m");
    assert!(vcx.debug_bounds("agent-row-preview-0-1").is_none());

    let card_p2 = kanban_rows
        .iter()
        .find(|row| row.card && row.key.pane == 2)
        .expect("exited gemini card rendered in Kanban");
    assert_eq!(card_p2.title, CLIAgent::Gemini.display_name());
    assert_eq!(card_p2.reason, "Exited");
    assert!(vcx.debug_bounds("agent-row-preview-0-2").is_none());

    let card_p3 = kanban_rows
        .iter()
        .find(|row| row.card && row.key.pane == 3)
        .expect("live claude card rendered in Kanban");
    assert_eq!(card_p3.title, CLIAgent::Claude.display_name());
    // A5.1: the card sits in the Working column, which already says it.
    assert_eq!(card_p3.reason, "");
    assert!(vcx.debug_bounds("agent-row-reason-0-3").is_none());
    assert!(vcx.debug_bounds("agent-row-preview-0-3").is_some());
}

/// Agents F6 on the drawn Inbox: one agent walked through every state with
/// real daemon reports, so each frame holds one row and at most one status
/// mark, and the shared `row-status-circle-0` selector names that row's mark
/// (in agent mode the rail is the projects list, which draws none). Working,
/// Waiting and Done carry the 5px dot centred in the 18px trailing slot;
/// Idle and Exited carry none and their text runs to the row's 8px padding.
/// Only Waiting is hollow, in the warning ink. Sizes are the finding's,
/// written out rather than read from the constants the row is drawn with.
#[gpui::test]
fn one_agent_through_every_state_dots_only_working_waiting_and_done(cx: &mut TestAppContext) {
    let _probe = crate::ui::paint_probe::scope();
    let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    let now_s = crate::core::config::unix_now();
    DaemonMsg::Agent(Some(CLIAgent::Claude))
        .encode(&mut streams[0])
        .unwrap();
    let sem = vcx.update(|_, cx| crate::ui::theme::semantics(cx));
    let eps = 0.01;
    // (reported status, exited, settled state, expected (fill, border) of the mark)
    let steps = [
        (
            AgentStatus::Working,
            false,
            State::Working,
            Some((Some(sem.info.ink), None)),
        ),
        (
            AgentStatus::Waiting,
            false,
            State::Waiting,
            Some((None, Some(sem.warning.ink))),
        ),
        (
            AgentStatus::Done,
            false,
            State::Done,
            Some((Some(sem.success.ink), None)),
        ),
        (AgentStatus::Idle, false, State::Idle, None),
        (AgentStatus::Idle, true, State::Exited, None),
    ];
    for (status, exited, want, mark) in steps {
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status,
            since_ms: Some(now_s.saturating_sub(60) * 1000),
            detection: Some(AgentDetectionSession {
                kind: "claude".into(),
                epoch: if exited { 2 } else { 1 },
                exited,
                source: None,
                rule: None,
            }),
            ..Default::default()
        }))
        .encode(&mut streams[0])
        .unwrap();
        if exited {
            DaemonMsg::Agent(None).encode(&mut streams[0]).unwrap();
        }
        let mut key = None;
        for _ in 0..200 {
            vcx.executor()
                .advance_clock(std::time::Duration::from_millis(50));
            vcx.run_until_parked();
            key = app.read_with(&vcx, |app, cx| {
                app.agent_pane_summaries(cx)
                    .iter()
                    .find(|p| p.state == want)
                    .map(|p| p.location.key)
            });
            if key.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let key = key.unwrap_or_else(|| panic!("the agent never settled as {want:?}"));

        app.update_in(&mut vcx, |app, window, cx| {
            app.set_agent_mode(ViewMode::Inbox, window, cx);
            app.agent_vision.state.show_other = true;
            app.working_dot_on = true;
            cx.notify();
        });
        for _ in 0..2 {
            vcx.run_until_parked();
            crate::ui::paint_probe::clear();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }

        // The one tab's agent is host 0, pane 1: the selectors are literals
        // because `debug_bounds` takes `&'static str`.
        assert_eq!(
            (key.host.0, key.pane),
            (0, 1),
            "{want:?}: the fixture's only agent"
        );
        let frame = vcx
            .debug_bounds("agent-row-frame-0-1")
            .unwrap_or_else(|| panic!("{want:?}: the row was painted"));
        let title = vcx
            .debug_bounds("agent-row-title-0-1")
            .unwrap_or_else(|| panic!("{want:?}: the title was painted"));
        let trailing = (frame.right() - title.right()).as_f32();
        let dot = vcx.debug_bounds("row-status-circle-0");
        let notes = crate::ui::paint_probe::all("row-status-dot");
        match mark {
            Some(paint) => {
                let dot = dot.unwrap_or_else(|| panic!("{want:?}: the row has its dot"));
                assert_eq!(
                    dot.size,
                    gpui::size(gpui::px(5.), gpui::px(5.)),
                    "{want:?}: a 5px dot"
                );
                let slot_centre = frame.right().as_f32() - 8. - 18. / 2.;
                assert!(
                    (dot.center().x.as_f32() - slot_centre).abs() < eps,
                    "{want:?}: centred in the 18px slot on the row's padding: {dot:?} in {frame:?}"
                );
                assert!(
                    (trailing - (8. + 18. + 8.)).abs() < eps,
                    "{want:?}: text stops before the slot and its gap, {trailing}"
                );
                assert_eq!(notes.len(), 1, "{want:?}: one mark: {notes:?}");
                assert_eq!(
                    (notes[0].fill, notes[0].border),
                    paint,
                    "{want:?}: filled in its role's ink, hollow only while waiting"
                );
            }
            None => {
                assert!(dot.is_none(), "{want:?}: no dot");
                assert!(notes.is_empty(), "{want:?}: no mark built: {notes:?}");
                assert!(
                    (trailing - 8.).abs() < eps,
                    "{want:?}: with no mark the text runs to the padding, {trailing}"
                );
            }
        }
    }
}

fn draw_inbox(vcx: &mut VisualTestContext) {
    let _ = take_rendered_headers();
    for _ in 0..2 {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
}

fn search(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, text: &str) {
    app.update_in(vcx, |app, window, cx| {
        let input = app.agent_vision.input.clone();
        input.update(cx, |input, cx| input.set_value(text, window, cx));
    });
}

/// A query nothing matches, over agents that exist and one of which is still
/// selected: the list says "No matching agents", never the false "No agents
/// need you here", and nothing is left open to read.
#[gpui::test]
fn an_inbox_search_with_no_match_says_so_and_opens_no_reader(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    assert!(
        app.update_in(&mut vcx, |app, _, _| app.agent_vision.state.selected())
            .is_some(),
        "the fixture starts with a selection that the query will filter out"
    );
    search(&app, &mut vcx, "zzzz");
    draw_inbox(&mut vcx);
    assert!(
        vcx.debug_bounds("agent-inbox-empty-VisionNoAgents")
            .is_some()
    );
    assert!(vcx.debug_bounds("agent-inbox-empty-VisionEmpty").is_none());
    assert!(
        vcx.debug_bounds("agent-reader-hint").is_none(),
        "there is nothing to select, so no instruction to select"
    );
    assert!(
        take_rendered_headers().is_empty(),
        "a stale selection must not keep a reader open beside an empty list"
    );
}

#[gpui::test]
fn a_query_that_matches_a_real_prompt_keeps_it_and_its_reader(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    search(&app, &mut vcx, "task 0");
    draw_inbox(&mut vcx);
    assert!(vcx.debug_bounds("agent-row-frame-0-1").is_some());
    assert!(
        vcx.debug_bounds("agent-inbox-empty-VisionNoAgents")
            .is_none()
    );
    assert!(vcx.debug_bounds("agent-inbox-empty-VisionEmpty").is_none());
    assert!(vcx.debug_bounds("agent-reader-hint").is_none());
    assert!(
        !take_rendered_headers().is_empty(),
        "the matching prompt is still readable"
    );
}

/// No agents at all: the existing empty message, and no reader hint.
#[gpui::test]
fn a_truly_empty_inbox_keeps_its_message_and_hides_the_reader_hint(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.set_agent_mode(ViewMode::Inbox, window, cx)
    });
    draw_inbox(&mut vcx);
    assert!(vcx.debug_bounds("agent-inbox-empty-VisionEmpty").is_some());
    assert!(
        vcx.debug_bounds("agent-inbox-empty-VisionNoAgents")
            .is_none()
    );
    assert!(vcx.debug_bounds("agent-reader-hint").is_none());
    assert!(take_rendered_headers().is_empty());
}

/// A project that holds none of the agents, with no query: the filter, not a
/// search, emptied the list, so the empty message stays and the stale
/// selection opens no reader. The project is a real pinned group: every
/// frame's refresh reconciles the filter against the projects that exist and
/// resets one that names none of them.
#[gpui::test]
fn a_project_filter_that_empties_the_inbox_keeps_its_message(cx: &mut TestAppContext) {
    use crate::core::group_key::{GroupKey, PinnedGroup};
    let (app, mut vcx, _streams) = agents(cx);
    let empty = PinnedGroup::label("Empty");
    let filter = super::ProjectFilter::Group(Some(GroupKey::Pinned(empty.id)));
    app.update_in(&mut vcx, |app, _, _| {
        app.sidebar_groups.pinned.push(empty);
        app.agent_vision.state.set_project_filter(filter.clone());
    });
    draw_inbox(&mut vcx);
    app.update_in(&mut vcx, |app, _, _| {
        assert_eq!(
            app.agent_vision.state.project_filter(),
            &filter,
            "the filter survived the frame's reconcile, so the list is empty because of it"
        );
    });
    assert!(vcx.debug_bounds("agent-inbox-empty-VisionEmpty").is_some());
    assert!(
        vcx.debug_bounds("agent-inbox-empty-VisionNoAgents")
            .is_none()
    );
    assert!(vcx.debug_bounds("agent-reader-hint").is_none());
    assert!(take_rendered_headers().is_empty());
}

/// Nothing needs attention but the query still matches agents in Other: the
/// list is not empty, so it keeps the true "No agents need you here".
#[gpui::test]
fn a_query_matching_only_other_agents_is_not_no_matching_agents(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents_in(cx, AgentStatus::Working);
    search(&app, &mut vcx, "codex");
    draw_inbox(&mut vcx);
    assert!(vcx.debug_bounds("agent-inbox-empty-VisionEmpty").is_some());
    assert!(
        vcx.debug_bounds("agent-inbox-empty-VisionNoAgents")
            .is_none()
    );
}

// ---- Agents F11: the keyboard help footer ---------------------------------
//
// The strip is selected by the REAL window focus (the reader terminal's
// handle, the list's handle, anything else), never by `state.focus()`, which
// is a mirror that only catches up on the next key. Selector names are the
// contract the footer builder must carry: `agent-help-footer` on the strip and
// `agent-help-<L10nKey>` on the text it shows. Expected strings are the
// English catalog written out here, not read back from `t()`.

const HELP_FOOTER: &str = "agent-help-footer";
const HELP_LIST: &str = "agent-help-VisionListHelp";
const HELP_BOARD: &str = "agent-help-VisionBoardHelp";
const HELP_TERMINAL: &str = "agent-help-VisionTerminalHelp";
const LIST_HELP_EN: &str = "↑ ↓ / j k select · Enter answer · [ ] project";
const BOARD_HELP_EN: &str = "← → / h l column · ↑ ↓ / j k card · Enter answer";
const TERMINAL_HELP_EN: &str = "Esc returns to the list";
/// The design system's status-bar height: the strip never gets shorter.
const FOOTER_MIN_H: f32 = 26.;

fn redraw(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |_, _, cx| cx.notify());
    draw_inbox(vcx);
}

fn shown_help(vcx: &mut VisualTestContext) -> Vec<&'static str> {
    [HELP_LIST, HELP_BOARD, HELP_TERMINAL]
        .into_iter()
        .filter(|key| vcx.debug_bounds(key).is_some())
        .collect()
}

fn viewport(vcx: &mut VisualTestContext) -> gpui::Size<gpui::Pixels> {
    vcx.update(|window, _| window.viewport_size())
}

/// The strip exists, is at least 26px, sits inside the window, and shows
/// exactly `want` with the laid-out text width of `english` (or, when the
/// string cannot fit, no wider than the strip).
fn assert_help(
    vcx: &mut VisualTestContext,
    want: &'static str,
    english: &str,
    what: &str,
) -> gpui::Bounds<gpui::Pixels> {
    assert_eq!(
        shown_help(vcx),
        vec![want],
        "{what}: the footer must show exactly {want}"
    );
    // The exact key and localized string the footer builder handed its text
    // child in the latest frame, against the literal English catalog text.
    assert_eq!(
        take_rendered_help().pop(),
        Some(Some(RenderedHelp {
            key: want.trim_start_matches("agent-help-").to_string(),
            text: english.to_string(),
        })),
        "{what}: the builder must pass {want} and the English string to the footer"
    );
    let strip = vcx
        .debug_bounds(HELP_FOOTER)
        .unwrap_or_else(|| panic!("{what}: the footer strip was not painted"));
    assert!(
        strip.size.height >= gpui::px(FOOTER_MIN_H),
        "{what}: the strip is {:?} high, under {FOOTER_MIN_H}px",
        strip.size.height
    );
    let size = viewport(vcx);
    assert!(
        strip.bottom() <= size.height + gpui::px(0.5),
        "{what}: the strip {strip:?} leaves the {size:?} window"
    );
    let text = vcx.debug_bounds(want).expect("checked above");
    assert!(
        text.left() >= strip.left() - gpui::px(0.5)
            && text.right() <= strip.right() + gpui::px(0.5)
            && text.top() >= strip.top() - gpui::px(0.5)
            && text.bottom() <= strip.bottom() + gpui::px(0.5),
        "{what}: the text {text:?} is not inside its strip {strip:?}"
    );
    // Layout/identity signal only: the selector names which help string was
    // built, and the shaped width of the literal English catalog text at the
    // approved 0.75rem on this window's rem and the installed UI font must
    // match the laid-out box. It does not read back the drawn glyphs, so it
    // is NOT proof of visible text content; that is the production-phase
    // builder capture plus native frames.
    let measured = vcx.update(|window, cx| {
        use gpui_component::ActiveTheme as _;
        let font = gpui::Font {
            family: cx.theme().font_family.clone(),
            features: gpui::FontFeatures::default(),
            fallbacks: None,
            weight: gpui::FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
        };
        let size = 0.75 * window.rem_size().as_f32();
        crate::ui::text_elision::measure_text(window.text_system(), &font, size, english)
    });
    let width = text.size.width.as_f32();
    if measured + 24. <= strip.size.width.as_f32() {
        assert!(
            (width - measured).abs() <= 1.,
            "{what}: laid-out box is {width}px, the English string shapes to {measured}px at 0.75rem"
        );
    } else {
        assert!(
            width <= strip.size.width.as_f32() + 0.5,
            "{what}: truncated text {width}px overflows the {:?} strip",
            strip.size.width
        );
    }
    strip
}

/// The strip is a full-width band under list and reader that never covers a
/// scroll area: each real scroll container ends at or above its top.
fn assert_clear_of_scroll(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    strip: gpui::Bounds<gpui::Pixels>,
    kanban: bool,
) {
    let size = viewport(vcx);
    assert!(
        (strip.right() - size.width).abs() <= gpui::px(0.5),
        "the strip {strip:?} must reach the window's right edge ({size:?})"
    );
    let scrolls: Vec<gpui::Bounds<gpui::Pixels>> = app.read_with(vcx, |app, _| {
        if kanban {
            let vision = &app.agent_vision;
            std::iter::once(vision.board_horizontal.bounds())
                .chain(vision.board_scroll.iter().map(|lane| lane.bounds()))
                .collect()
        } else {
            vec![app.agent_vision.inbox_scroll.bounds()]
        }
    });
    for scroll in &scrolls {
        assert!(
            scroll.size.height > gpui::px(0.),
            "the scroll area was not laid out: {scroll:?}"
        );
        assert!(
            scroll.bottom() <= strip.top() + gpui::px(0.5),
            "the strip {strip:?} covers scroll content {scroll:?}"
        );
        assert!(
            strip.left() <= scroll.left() + gpui::px(0.5),
            "the strip {strip:?} starts right of scroll content {scroll:?}"
        );
    }
    if !kanban {
        assert!(
            strip.right() > scrolls[0].right() + gpui::px(100.),
            "under list AND reader means wider than the list column: {strip:?} vs {:?}",
            scrolls[0]
        );
    }
}

#[gpui::test]
fn footer_follows_real_list_terminal_list_keys_in_the_inbox(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    redraw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, _| {
        assert!(app.agent_vision.list_focus.is_focused(window));
    });
    let strip = assert_help(&mut vcx, HELP_LIST, LIST_HELP_EN, "list focused");
    assert_clear_of_scroll(&app, &mut vcx, strip, false);

    vcx.simulate_keystrokes("enter");
    redraw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        let key = app.agent_vision.state.selected().unwrap();
        assert!(
            app.agent_terminal(key, cx)
                .unwrap()
                .focus_handle(cx)
                .contains_focused(window, cx),
            "Enter must really focus the reader terminal"
        );
    });
    let strip = assert_help(
        &mut vcx,
        HELP_TERMINAL,
        TERMINAL_HELP_EN,
        "terminal focused",
    );
    assert_clear_of_scroll(&app, &mut vcx, strip, false);

    // A plain key goes to the terminal; the footer must not flip back.
    vcx.simulate_keystrokes("j");
    redraw(&app, &mut vcx);
    assert_help(
        &mut vcx,
        HELP_TERMINAL,
        TERMINAL_HELP_EN,
        "terminal after a plain key",
    );

    vcx.simulate_keystrokes("escape");
    redraw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, _| {
        assert!(
            app.agent_vision.list_focus.is_focused(window),
            "Escape must really return focus to the list"
        );
    });
    let strip = assert_help(&mut vcx, HELP_LIST, LIST_HELP_EN, "back on the list");
    assert_clear_of_scroll(&app, &mut vcx, strip, false);
}

/// What a click does: the window's focus moves while the `state.focus` mirror
/// stays where the last key left it. A footer keyed to the mirror is wrong in
/// every step below.
#[gpui::test]
fn footer_reads_the_window_focus_not_the_mirror(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    redraw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        let key = app.agent_vision.state.selected().unwrap();
        let terminal = app.agent_terminal(key, cx).unwrap();
        window.focus(&terminal.focus_handle(cx), cx);
        assert_eq!(
            app.agent_vision.state.focus(),
            FocusZone::List,
            "the fixture must leave the mirror stale"
        );
    });
    redraw(&app, &mut vcx);
    assert_help(
        &mut vcx,
        HELP_TERMINAL,
        TERMINAL_HELP_EN,
        "terminal focused by click, mirror says List",
    );

    // Search owns plain keys: no list help would be true there. The strip
    // keeps its height so nothing jumps, and says nothing.
    app.update_in(&mut vcx, |app, window, cx| {
        window.focus(&app.agent_vision.input.focus_handle(cx), cx);
    });
    redraw(&app, &mut vcx);
    let shown = shown_help(&mut vcx);
    assert!(
        shown.is_empty(),
        "no help text while focus is outside list and reader: {shown:?}"
    );
    assert_eq!(
        take_rendered_help().pop(),
        Some(None),
        "the builder must hand the footer no text while focus is elsewhere"
    );
    let strip = vcx
        .debug_bounds(HELP_FOOTER)
        .expect("the strip keeps its reserved height with no text");
    assert!(strip.size.height >= gpui::px(FOOTER_MIN_H));

    app.update_in(&mut vcx, |app, window, cx| {
        window.focus(&app.agent_vision.list_focus, cx);
    });
    redraw(&app, &mut vcx);
    assert_help(&mut vcx, HELP_LIST, LIST_HELP_EN, "list refocused");
}

#[gpui::test]
fn footer_shows_board_help_in_kanban_and_enter_opens_the_terminal_help(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.set_agent_mode(ViewMode::Kanban, window, cx)
    });
    redraw(&app, &mut vcx);
    let strip = assert_help(&mut vcx, HELP_BOARD, BOARD_HELP_EN, "kanban");
    assert_clear_of_scroll(&app, &mut vcx, strip, true);

    vcx.simulate_keystrokes("enter");
    redraw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.agent_vision.state.mode(), ViewMode::Inbox);
        let key = app.agent_vision.state.selected().unwrap();
        assert!(
            app.agent_terminal(key, cx)
                .unwrap()
                .focus_handle(cx)
                .contains_focused(window, cx)
        );
    });
    assert_help(
        &mut vcx,
        HELP_TERMINAL,
        TERMINAL_HELP_EN,
        "kanban Enter opens the reader",
    );

    vcx.simulate_keystrokes("escape");
    redraw(&app, &mut vcx);
    assert_help(
        &mut vcx,
        HELP_LIST,
        LIST_HELP_EN,
        "Escape lands on the Inbox list",
    );
}

#[gpui::test]
fn footer_stays_under_a_truly_empty_inbox(cx: &mut TestAppContext) {
    // No agents at all.
    let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.set_agent_mode(ViewMode::Inbox, window, cx)
    });
    redraw(&app, &mut vcx);
    assert!(vcx.debug_bounds("agent-inbox-empty-VisionEmpty").is_some());
    assert!(vcx.debug_bounds("agent-reader-hint").is_none());
    let strip = assert_help(&mut vcx, HELP_LIST, LIST_HELP_EN, "no agents");
    assert_clear_of_scroll(&app, &mut vcx, strip, false);
}

#[gpui::test]
fn footer_stays_under_a_search_with_no_match_and_an_emptying_filter(cx: &mut TestAppContext) {
    use crate::core::group_key::{GroupKey, PinnedGroup};
    let (app, mut vcx, _streams) = agents(cx);
    search(&app, &mut vcx, "zzzz");
    redraw(&app, &mut vcx);
    assert!(
        vcx.debug_bounds("agent-inbox-empty-VisionNoAgents")
            .is_some()
    );
    assert!(vcx.debug_bounds("agent-reader-hint").is_none());
    let strip = assert_help(&mut vcx, HELP_LIST, LIST_HELP_EN, "search with no match");
    assert_clear_of_scroll(&app, &mut vcx, strip, false);

    search(&app, &mut vcx, "");
    let empty = PinnedGroup::label("Empty");
    let filter = super::ProjectFilter::Group(Some(GroupKey::Pinned(empty.id)));
    app.update_in(&mut vcx, |app, _, _| {
        app.sidebar_groups.pinned.push(empty);
        app.agent_vision.state.set_project_filter(filter.clone());
    });
    redraw(&app, &mut vcx);
    assert!(vcx.debug_bounds("agent-inbox-empty-VisionEmpty").is_some());
    assert!(vcx.debug_bounds("agent-reader-hint").is_none());
    let strip = assert_help(&mut vcx, HELP_LIST, LIST_HELP_EN, "project filter empties");
    assert_clear_of_scroll(&app, &mut vcx, strip, false);
}

/// Feasibility sweep, not a footer assertion: the footer's ink is the installed
/// `muted_foreground`; its fill is the workspace root fill the theme installs
/// as `ActiveBackground` (never `theme.background`). Every builtin and
/// generated theme at the default config, in three separate classes that are
/// never merged: PROVEN solid fills (one exact contrast), SAMPLED gradients
/// (17 points per axis in sRGB: bounded diagnostic evidence, no monotonic or
/// full-gradient claim), and UNSUPPORTED themes (a background image, or alpha
/// below 1 on the fill or the ink), which tokens cannot prove, are listed by
/// id and skipped, and never count as passes.
#[gpui::test]
fn muted_help_ink_against_the_actual_workspace_fill_for_every_default_theme(
    cx: &mut TestAppContext,
) {
    use gpui_component::ActiveTheme as _;
    crate::core::config::pin_test_config_dir();
    let mut proven_solid = 0usize;
    let mut sampled_gradient = 0usize;
    let mut unsupported: Vec<String> = Vec::new();
    let mut misses: Vec<String> = Vec::new();
    cx.update(|cx| {
        gpui_component::init(cx);
        let mut themes = crate::ui::presets::builtins();
        themes.extend(crate::ui::presets::generated_theme_grid());
        cx.set_global(crate::ui::presets::Themes(themes.clone()));
        for theme in &themes {
            cx.set_global(crate::core::config::Config(
                crate::core::config::CoreConfig {
                    theme_follow_system: false,
                    theme_preset: theme.id.clone(),
                    ..Default::default()
                },
            ));
            crate::ui::theme::apply_theme(None, cx);
            let ink = crate::ui::paint_probe::hsla24(cx.theme().muted_foreground);
            let ink_alpha = cx.theme().muted_foreground.a;
            let active = cx.global::<crate::ui::presets::ActiveBackground>();
            let poles: Vec<(u32, u32)> = match active.fill {
                crate::ui::presets::Fill::Solid(c) => vec![(c, c)],
                crate::ui::presets::Fill::Vertical { top, bottom } => vec![(top, bottom)],
                crate::ui::presets::Fill::Horizontal { left, right } => vec![(left, right)],
            };
            if active.image.is_some() || active.opacity.is_some_and(|o| o < 1.) || ink_alpha < 1. {
                unsupported.push(theme.id.clone());
                continue;
            }
            for (a, b) in poles {
                let kind = if a == b {
                    proven_solid += 1;
                    "solid"
                } else {
                    sampled_gradient += 1;
                    "sampled gradient"
                };
                for step in 0..=16u32 {
                    let t = step as f32 / 16.;
                    let mix = |shift: u32| {
                        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
                        (x + (y - x) * t).round() as u32
                    };
                    let fill = mix(16) << 16 | mix(8) << 8 | mix(0);
                    let ratio = crate::ui::presets::contrast(ink, fill);
                    if ratio < 4.5 {
                        misses.push(format!(
                            "{} ({kind}) ink {ink:06x} on {fill:06x} = {ratio:.2}",
                            theme.id
                        ));
                        break;
                    }
                }
            }
        }
    });
    eprintln!(
        "F11 muted-ink sweep: {proven_solid} solid fills, {sampled_gradient} sampled gradient axes, {} unsupported themes skipped (ids: {:?}), {} misses",
        unsupported.len(),
        &unsupported[..unsupported.len().min(20)],
        misses.len()
    );
    assert!(
        misses.is_empty(),
        "{} misses ({proven_solid} solid fills, {sampled_gradient} sampled gradient axes; {} unsupported themes skipped, not passed): {:?}",
        misses.len(),
        unsupported.len(),
        &misses[..misses.len().min(20)]
    );
}

// ---- Agents search well: the shared keyboard ring around the existing 28px well ---------------------------------------
//
// The ring is the shared `FocusRing` (Inside, the well's own 7px radius) on the input's own focus handle, tagged `agent-search-ring`:
// the ring is `agent-search-ring` and its always-mounted wrapper `agent-search-ring-frame`. The well itself carries `agent-search-well`
// under cfg(test), a mount control, so a missing ring fails on its own assertion and never on a missing well. The focus is moved with
// real `window.focus` calls, the way a click or a Tab does; the footer is the unchanged `agent-help-*` strip.

#[gpui::test]
fn the_search_input_draws_the_shared_ring_inside_its_well_only_while_it_has_focus(
    cx: &mut TestAppContext,
) {
    use gpui_component::ActiveTheme as _;
    let (app, mut vcx, _streams) = agents(cx);
    redraw(&app, &mut vcx);
    let well = vcx
        .debug_bounds("agent-search-well")
        .expect("control: the search well is mounted");
    assert_eq!(
        well.size.height,
        gpui::px(28.),
        "control: the well keeps its 28px height"
    );
    assert!(
        vcx.debug_bounds("agent-search-ring").is_none(),
        "no ring while the list has the focus"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        window.focus(&app.agent_vision.input.focus_handle(cx), cx);
    });
    redraw(&app, &mut vcx);
    let ring = vcx
        .debug_bounds("agent-search-ring")
        .expect("the focused search input must draw its ring (agent-search-ring)");
    let gap = vcx.update(|_, cx| cx.theme().focus_ring_offset());
    let eps = gpui::px(0.5);
    // Inside: the ring is the well inset by the installed gap on every side
    for (side, got, want) in [
        ("left", ring.left(), well.left() + gap),
        ("top", ring.top(), well.top() + gap),
        ("right", ring.right(), well.right() - gap),
        ("bottom", ring.bottom(), well.bottom() - gap),
    ] {
        assert!(
            (got - want).abs() <= eps,
            "the ring's {side} edge is {got:?}, the well inset by the installed gap {gap:?} is {want:?} (well {well:?}, ring {ring:?})"
        );
    }
    // its always-mounted wrapper is the well's own box: nothing moved
    let frame = vcx
        .debug_bounds("agent-search-ring-frame")
        .expect("the ring's wrapper is mounted around the well");
    for (side, got, want) in [
        ("left", frame.left(), well.left()),
        ("top", frame.top(), well.top()),
        ("right", frame.right(), well.right()),
        ("bottom", frame.bottom(), well.bottom()),
    ] {
        assert!(
            (got - want).abs() <= eps,
            "the ring wrapper's {side} edge is {got:?}, the well's is {want:?}"
        );
    }
}

#[gpui::test]
fn the_search_ring_follows_the_window_focus_and_never_marks_the_list_the_reader_or_the_footer(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = agents(cx);
    redraw(&app, &mut vcx);
    assert!(
        vcx.debug_bounds("agent-search-well").is_some(),
        "control: the search well is mounted"
    );
    assert!(
        vcx.debug_bounds("agent-search-ring").is_none(),
        "no search ring while the list has the focus"
    );
    // the reader terminal has the focus (what a click does)
    app.update_in(&mut vcx, |app, window, cx| {
        let key = app.agent_vision.state.selected().unwrap();
        let terminal = app.agent_terminal(key, cx).unwrap();
        window.focus(&terminal.focus_handle(cx), cx);
    });
    redraw(&app, &mut vcx);
    assert!(
        vcx.debug_bounds("agent-search-ring").is_none(),
        "no search ring while the reader terminal has the focus"
    );
    // the search input has the focus: its ring, and the footer says nothing
    app.update_in(&mut vcx, |app, window, cx| {
        window.focus(&app.agent_vision.input.focus_handle(cx), cx);
    });
    redraw(&app, &mut vcx);
    assert!(
        vcx.debug_bounds("agent-search-ring").is_some(),
        "the search input must draw its ring (agent-search-ring) once it has the focus"
    );
    let shown = shown_help(&mut vcx);
    assert!(
        shown.is_empty(),
        "no help text while the search input owns the focus: {shown:?}"
    );
    // back on the list: the ring is gone and the list help is back
    app.update_in(&mut vcx, |app, window, cx| {
        window.focus(&app.agent_vision.list_focus, cx);
    });
    redraw(&app, &mut vcx);
    assert!(
        vcx.debug_bounds("agent-search-ring").is_none(),
        "no search ring once the list has the focus again"
    );
    assert_eq!(
        shown_help(&mut vcx),
        vec![HELP_LIST],
        "the list help is back with the list focused"
    );
}

// ---- Other toggle: Enter on the control's own Tab stop ---------------------
//
// The Other group's toggle is a library Button: a Tab stop that gpui clicks on
// the KEY UP of Enter or Space while it is focused. The list's capture-phase
// key handler must leave Enter to that focused control.

fn other_open(app: &Entity<Tty59App>, vcx: &VisualTestContext) -> bool {
    app.read_with(vcx, |app, _| app.agent_vision.state.show_other)
}

/// A key as a user presses it: the key down, then the key up. gpui turns a
/// focused element's Enter or Space into a click on the KEY UP, and only while
/// the element was focused when it was last painted.
///
/// Raw platform events with no `key_char`, so no simulated input method turns
/// a Space into text while a control is being probed.
fn press(vcx: &mut VisualTestContext, key: &str) {
    let mut keystroke = gpui::Keystroke::parse(key).expect("a key");
    keystroke.key_char = None;
    vcx.simulate_event(gpui::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    vcx.simulate_event(gpui::KeyUpEvent { keystroke });
    vcx.run_until_parked();
}

/// A clean starting point for one keyboard attempt: the Inbox, Other closed,
/// an empty query, focus on the list, then `steps` Tab stops forward, painted.
fn tab_stop_attempt(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, steps: usize) {
    app.update_in(vcx, |app, window, cx| {
        app.set_agent_mode(ViewMode::Inbox, window, cx);
        app.agent_vision.state.show_other = false;
        let input = app.agent_vision.input.clone();
        input.update(cx, |input, cx| input.set_value("", window, cx));
    });
    draw_inbox(vcx);
    app.update_in(vcx, |app, window, cx| {
        window.focus(&app.agent_vision.list_focus, cx);
        for _ in 0..steps {
            window.focus_next(cx);
        }
    });
    // Painted, so the focused element knows it is focused when the key comes up.
    draw_inbox(vcx);
}

/// The number of Tab stops from the list, counted from the closed state, at
/// which Space opens the Other group, or `None` when no stop within the first
/// 24 does. Only Space discovers it: an Enter pressed on an arbitrary earlier
/// control can do anything that control does.
fn toggle_tab_stop(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Option<usize> {
    (1..=24).find(|&steps| {
        tab_stop_attempt(app, vcx, steps);
        press(vcx, "space");
        other_open(app, vcx)
    })
}

/// The same control is activated by Enter: from the closed state, the stop
/// Space opened the group at is replayed, painted, and Enter pressed there.
#[gpui::test]
fn a51_other_toggle_enter_opens_the_group_from_the_space_tab_stop(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents_in(cx, AgentStatus::Working);
    let steps = toggle_tab_stop(&app, &mut vcx).expect("Space finds the control's Tab stop");
    tab_stop_attempt(&app, &mut vcx, steps);
    assert!(!other_open(&app, &vcx), "the replay starts closed");
    press(&mut vcx, "enter");
    assert!(
        other_open(&app, &vcx),
        "Enter on the control's Tab stop ({steps}) did not open the Other group"
    );
}

/// The keyboard runs the same callback as the click, in both directions: the
/// control's own focus handle (the one focused when Space opened the group) is
/// kept, and focusing that same control again lets Enter close it. Each press
/// hands focus to the list and the Other rows follow the state.
#[gpui::test]
fn a51_other_toggle_keyboard_matches_the_click_callback(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = agents_in(cx, AgentStatus::Working);
    let steps = toggle_tab_stop(&app, &mut vcx).expect("Space finds the control's Tab stop");
    tab_stop_attempt(&app, &mut vcx, steps);
    let control = vcx
        .update(|window, cx| window.focused(cx))
        .expect("a control is focused at the Tab stop");
    assert!(
        vcx.debug_bounds("agent-row-frame-0-1").is_none(),
        "the Other rows start hidden"
    );
    press(&mut vcx, "space");
    draw_inbox(&mut vcx);
    assert!(other_open(&app, &vcx), "Space opens");
    assert!(
        vcx.debug_bounds("agent-row-frame-0-1").is_some(),
        "the Other rows appear once it is open"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(
            app.agent_vision.list_focus.contains_focused(window, cx),
            "the list owns focus after Space, as after a click"
        );
        window.focus(&control, cx);
    });
    draw_inbox(&mut vcx);
    press(&mut vcx, "enter");
    draw_inbox(&mut vcx);
    assert!(!other_open(&app, &vcx), "Enter on the same control closes");
    assert!(
        vcx.debug_bounds("agent-row-frame-0-1").is_none(),
        "the Other rows disappear again"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(
            app.agent_vision.list_focus.contains_focused(window, cx),
            "the list owns focus after Enter, as after a click"
        );
    });
}

// ---------------------------------------------------------------------------
// A5.1: agent identity, deduplicated content and aligned section edges.
//
// Every test drives the real collector, the real daemon stream and the real
// Inbox/board render; none restates a constant of the row. Tests that carry
// several independent assertions collect them and fail once with all of them,
// so an early mismatch never hides the rest. Positive controls (what must
// stay) live in their own functions, apart from the cases that are red today.
// ---------------------------------------------------------------------------

use super::render::{AgentLine, RenderedAgentLine, RenderedAgentRow, take_rendered_lines};
use crate::daemon::transport::Stream;

/// Puts the locale back whatever happens in the test.
struct Locale;

impl Locale {
    fn set(code: &str) -> Self {
        crate::ui::i18n::set_locale(code);
        Self
    }
}

impl Drop for Locale {
    fn drop(&mut self) {
        crate::ui::i18n::set_locale("en");
    }
}

fn since_ms(secs_ago: u64) -> Option<u64> {
    Some(crate::core::config::unix_now().saturating_sub(secs_ago) * 1000)
}

/// One daemon report: the agent, its session state and, when `exited`, the
/// shell the pane falls back to.
fn report(
    stream: &mut Stream,
    agent: CLIAgent,
    status: AgentStatus,
    message: Option<&str>,
    exited: bool,
) {
    report_kind(stream, Some(agent), agent.slug(), status, message, exited);
}

fn report_kind(
    stream: &mut Stream,
    agent: Option<CLIAgent>,
    kind: &str,
    status: AgentStatus,
    message: Option<&str>,
    exited: bool,
) {
    if let Some(agent) = agent {
        DaemonMsg::Agent(Some(agent)).encode(stream).unwrap();
    }
    DaemonMsg::AgentStatus(Some(AgentSessionState {
        status,
        since_ms: since_ms(if exited { 180 } else { 60 }),
        message: message.map(str::to_string),
        detection: Some(AgentDetectionSession {
            kind: kind.into(),
            epoch: if exited { 2 } else { 1 },
            exited,
            source: None,
            rule: None,
        }),
        ..Default::default()
    }))
    .encode(stream)
    .unwrap();
    if exited {
        DaemonMsg::Agent(None).encode(stream).unwrap();
    }
}

/// Advances the clock and the daemon streams until `ready`, or fails naming
/// what never settled. A fixture that does not settle is a setup failure.
fn wait_for(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    what: &str,
    ready: impl Fn(&Tty59App, &gpui::App) -> bool,
) {
    for _ in 0..200 {
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        vcx.run_until_parked();
        if app.read_with(&*vcx, |app, cx| ready(app, cx)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the fixture never settled: {what}");
}

fn draw_frames(vcx: &mut VisualTestContext) {
    for _ in 0..2 {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
}

/// What one settled frame pair handed to its row elements.
struct Frame {
    rows: Vec<RenderedAgentRow>,
    lines: Vec<RenderedAgentLine>,
}

fn rendered_rows(vcx: &mut VisualTestContext) -> Frame {
    draw_frames(vcx);
    let _ = take_rendered_rows();
    let _ = take_rendered_lines();
    draw_frames(vcx);
    Frame {
        rows: take_rendered_rows(),
        lines: take_rendered_lines(),
    }
}

fn row_of(frame: &Frame, card: bool, pane: u64) -> RenderedAgentRow {
    frame
        .rows
        .iter()
        .rfind(|row| row.card == card && row.key.pane == pane)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "no {} rendered for pane {pane}: {:?}",
                if card { "card" } else { "row" },
                frame.rows
            )
        })
}

/// The text a row handed to its meta or preview element, or `None` when it
/// built no such element.
fn line_of(frame: &Frame, card: bool, pane: u64, line: AgentLine) -> Option<String> {
    frame
        .lines
        .iter()
        .rfind(|l| l.card == card && l.key.pane == pane && l.line == line)
        .map(|l| l.text.clone())
}

/// Shows `mode` with the Other group open and `selected` (a pane id) selected.
fn show(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    mode: ViewMode,
    selected: Option<u64>,
) {
    app.update_in(vcx, |app, window, cx| {
        app.set_agent_mode(mode, window, cx);
        app.agent_vision.state.show_other = true;
        if let Some(pane) = selected {
            // The collector is the real source the view's own refresh copies
            // from; the cached `agent_vision.panes` is not refreshed until a
            // frame, so selecting from it would race the draw.
            let key = app
                .agent_pane_summaries(cx)
                .iter()
                .find(|p| p.location.key.pane == pane)
                .unwrap_or_else(|| panic!("no agent pane {pane}"))
                .location
                .key;
            app.agent_vision.state.focus_list_item(key);
        }
        cx.notify();
    });
    vcx.run_until_parked();
}

fn project_of(app: &Entity<Tty59App>, vcx: &VisualTestContext, pane: u64) -> String {
    app.read_with(vcx, |app, _| {
        let group = app
            .agent_vision
            .panes
            .iter()
            .find(|p| p.location.key.pane == pane)
            .unwrap_or_else(|| panic!("no agent pane {pane}"))
            .location
            .group
            .clone();
        app.agent_vision
            .projects
            .iter()
            .find(|project| project.group == group)
            .map_or(String::new(), |project| project.name.clone())
    })
}

fn near(a: gpui::Pixels, b: gpui::Pixels) -> bool {
    (a - b).as_f32().abs() < 0.5
}

fn fail_if(failures: &mut Vec<String>, broken: bool, message: impl FnOnce() -> String) {
    if broken {
        failures.push(message());
    }
}

fn assert_no_failures(failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} expectation(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A window with three agents and one plain shell:
///
/// - tab 0 is a split `[pane 1 Claude Working | shell 10 | pane 9 Codex
///   Waiting "Allow Bash: pnpm test"]`, so the Codex pane is leaf 3 of 3 but
///   the second AGENT;
/// - tab 1 is a single pane (2) running Codex, Idle.
///
/// Streams: `[0]` pane 1, `[1]` pane 2, `[2]` shell, `[3]` pane 9.
fn mixed_split(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext, Vec<Stream>) {
    mixed_split_at(cx, None)
}

/// `mixed_split` with the UI font size set to `ui_size` when given.
fn mixed_split_at(
    cx: &mut TestAppContext,
    ui_size: Option<f32>,
) -> (Entity<Tty59App>, VisualTestContext, Vec<Stream>) {
    use crate::terminal::view::quiet_test_pane;
    use crate::ui::pane::{Pane, PaneSlot};
    let (app, mut vcx, mut streams) =
        crate::ui::app::test_window::harness_with_tabs_configured(cx, 2, |config| {
            if let Some(ui_size) = ui_size {
                config.ui_font_size = ui_size;
            }
        });
    app.update_in(&mut vcx, |app, window, cx| {
        let (shell, shell_stream) = quiet_test_pane(10, window, cx);
        let (sibling, sibling_stream) = quiet_test_pane(9, window, cx);
        streams.push(shell_stream);
        streams.push(sibling_stream);
        let first = app.tabs[0].pane.first_leaf().unwrap();
        app.tabs[0].pane = Pane::split_node(
            gpui::Axis::Horizontal,
            0.5,
            Pane::leaf(first),
            Pane::split_node(
                gpui::Axis::Vertical,
                0.5,
                Pane::leaf(PaneSlot::Ready(shell)),
                Pane::leaf(PaneSlot::Ready(sibling)),
            ),
        );
    });
    report(
        &mut streams[0],
        CLIAgent::Claude,
        AgentStatus::Working,
        None,
        false,
    );
    report(
        &mut streams[1],
        CLIAgent::Codex,
        AgentStatus::Idle,
        None,
        false,
    );
    report(
        &mut streams[3],
        CLIAgent::Codex,
        AgentStatus::Waiting,
        Some("Allow Bash: pnpm test"),
        false,
    );
    wait_for(&app, &mut vcx, "three agent panes", |app, cx| {
        app.agent_pane_summaries(cx).len() == 3
    });
    (app, vcx, streams)
}

fn claude() -> &'static str {
    CLIAgent::Claude.display_name()
}

fn codex() -> &'static str {
    CLIAgent::Codex.display_name()
}

fn title_of(app: &Entity<Tty59App>, vcx: &VisualTestContext, pane: u64) -> String {
    app.read_with(vcx, |app, cx| {
        app.agent_pane_summaries(cx)
            .iter()
            .find(|p| p.location.key.pane == pane)
            .unwrap_or_else(|| panic!("no agent pane {pane}"))
            .title()
            .to_string()
    })
}

// ----- F2: identity ---------------------------------------------------------

/// Control: a split's positions count every leaf, the shell included, so the
/// Codex pane beside a shell is 3/3 and not the second agent's 2/2.
#[gpui::test]
fn a51_split_numbering_counts_every_leaf(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, vcx, _streams) = mixed_split(cx);
    app.read_with(&vcx, |app, cx| {
        let panes = app.agent_pane_summaries(cx);
        let position = |pane: u64| {
            panes
                .iter()
                .find(|p| p.location.key.pane == pane)
                .unwrap()
                .pane_position
        };
        assert_eq!(position(1), Some((1, 3)));
        assert_eq!(position(9), Some((3, 3)));
        assert_eq!(position(2), None, "a single-pane tab has no marker");
    });
}

/// Every pane of a split is titled by ITS agent, not by the tab's label,
/// which follows whichever pane the tab names itself after.
#[gpui::test]
fn a51_split_collector_titles_each_pane_by_its_own_agent(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, vcx, _streams) = mixed_split(cx);
    let mut failures = Vec::new();
    let (first, second) = (title_of(&app, &vcx, 1), title_of(&app, &vcx, 9));
    fail_if(&mut failures, first != claude(), || {
        format!("pane 1 title {first:?}, want {:?}", claude())
    });
    fail_if(&mut failures, second != codex(), || {
        format!("pane 9 title {second:?}, want {:?}", codex())
    });
    assert_no_failures(failures);
}

/// A custom tab name and a terminal title do not make every pane of a split
/// share one label: each pane keeps its own agent's name either way.
#[gpui::test]
fn a51_named_and_titled_splits_still_title_each_pane_by_its_agent(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = mixed_split(cx);
    let mut failures = Vec::new();

    app.update_in(&mut vcx, |app, _, _| {
        app.tabs[0].name = Some("Release".into());
    });
    for (pane, want) in [(1, claude()), (9, codex())] {
        let got = title_of(&app, &vcx, pane);
        fail_if(&mut failures, got != want, || {
            format!("named split: pane {pane} title {got:?}, want {want:?}")
        });
    }

    app.update_in(&mut vcx, |app, _, cx| {
        app.tabs[0].name = None;
        let leaf = app.tabs[0].pane.first_leaf().unwrap();
        leaf.terminal()
            .unwrap()
            .clone()
            .update(cx, |view, _| view.title = "Fix auth".into());
    });
    for (pane, want) in [(1, claude()), (9, codex())] {
        let got = title_of(&app, &vcx, pane);
        fail_if(&mut failures, got != want, || {
            format!("titled split: pane {pane} title {got:?}, want {want:?}")
        });
    }
    assert_no_failures(failures);
}

/// Control: a single-pane tab keeps its tab label, custom or not, and the
/// agent's name stays in its meta line because the title does not supply it.
#[gpui::test]
fn a51_single_pane_tab_keeps_its_custom_label(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = mixed_split(cx);
    app.update_in(&mut vcx, |app, _, _| {
        app.tabs[1].name = Some("Review auth".into());
    });
    assert_eq!(title_of(&app, &vcx, 2), "Review auth");
    show(&app, &mut vcx, ViewMode::Inbox, None);
    let rows = rendered_rows(&mut vcx);
    let row = row_of(&rows, false, 2);
    let project = project_of(&app, &vcx, 2);
    let age = crate::ui::home::compact_age(60);
    assert_eq!(row.title, "Review auth");
    assert_eq!(
        line_of(&rows, false, 2, AgentLine::Meta),
        Some(format!("{} · {project} · {age}", codex())),
        "a custom title does not name the agent, so the meta keeps it"
    );
}

/// The Inbox rows and the board's cards title each pane by its own agent.
#[gpui::test]
fn a51_split_inbox_and_kanban_titles_follow_each_pane(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = mixed_split(cx);
    let mut failures = Vec::new();
    for (mode, card) in [(ViewMode::Inbox, false), (ViewMode::Kanban, true)] {
        show(&app, &mut vcx, mode, None);
        let rows = rendered_rows(&mut vcx);
        for (pane, want) in [(1, claude()), (9, codex())] {
            let got = row_of(&rows, card, pane).title;
            fail_if(&mut failures, got != want, || {
                format!("{mode:?}: pane {pane} drawn title {got:?}, want {want:?}")
            });
        }
    }
    assert_no_failures(failures);
}

/// The marker is its own muted span beside the title on the same line (the
/// title element carries only the name) and the reader's heading, which joins
/// them, names the Codex pane with its leaf number among ALL leaves.
#[gpui::test]
fn a51_split_marker_is_a_separate_span_and_the_reader_names_the_pane(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = mixed_split(cx);
    show(&app, &mut vcx, ViewMode::Inbox, Some(9));
    let _ = take_rendered_headers();
    let _ = rendered_rows(&mut vcx);
    let headers = take_rendered_headers();
    let mut failures = Vec::new();
    let reader = headers.iter().rfind(|header| header.key.pane == 9);
    fail_if(&mut failures, reader.is_none(), || {
        "no reader header for pane 9".into()
    });
    if let Some(reader) = reader {
        let want = format!("{} · pane 3/3", codex());
        fail_if(&mut failures, reader.title != want, || {
            format!("reader heading {:?}, want {want:?}", reader.title)
        });
    }
    match (
        vcx.debug_bounds("agent-row-title-0-9"),
        vcx.debug_bounds("agent-row-marker-0-9"),
    ) {
        (Some(title), Some(marker)) => {
            fail_if(
                &mut failures,
                marker.left() < title.right() - gpui::px(0.5),
                || format!("marker {marker:?} is not after the title {title:?}"),
            );
            fail_if(
                &mut failures,
                (marker.center().y - title.center().y).as_f32().abs() > title.size.height.as_f32(),
                || format!("marker {marker:?} is not on the title's line {title:?}"),
            );
        }
        (title, marker) => failures.push(format!(
            "title {title:?} and a separate marker {marker:?} must both be drawn"
        )),
    }
    assert_no_failures(failures);
}

// ----- F2: the name and the marker in a narrow card ---------------------------
//
// The marker (`панель 3/3`) is its own span beside the name. When the card is
// as narrow as the board lets it get (210px) and the text as large as the UI
// goes (24), the two cannot share a line, and the name must not be what gives:
// it keeps the width its text needs (up to what the card has), and the marker
// either stays beside it or moves to the line under it, whole and inside the
// card. (Widths are measured with the test platform's fixed-metric text
// system, which the layout also uses; they are not native pixels, so the
// native ru/UI24 frames stay the proof of the same defect on real fonts.)

/// The four selectors of one card's drawn pieces: frame, avatar, name, marker.
fn card_selectors(pane: u64) -> [&'static str; 4] {
    match pane {
        1 => [
            "agent-row-frame-0-1",
            "agent-row-icon-0-1",
            "agent-row-title-0-1",
            "agent-row-marker-0-1",
        ],
        2 => [
            "agent-row-frame-0-2",
            "agent-row-icon-0-2",
            "agent-row-title-0-2",
            "agent-row-marker-0-2",
        ],
        9 => [
            "agent-row-frame-0-9",
            "agent-row-icon-0-9",
            "agent-row-title-0-9",
            "agent-row-marker-0-9",
        ],
        other => panic!("no card selectors for pane {other}"),
    }
}

type Bounds = gpui::Bounds<gpui::Pixels>;

struct CardPieces {
    card: Bounds,
    icon: Bounds,
    title: Bounds,
    marker: Option<Bounds>,
}

fn card_pieces(vcx: &mut VisualTestContext, pane: u64) -> Result<CardPieces, String> {
    let [frame, icon, title, marker] = card_selectors(pane);
    let (Some(card), Some(icon), Some(title)) = (
        vcx.debug_bounds(frame),
        vcx.debug_bounds(icon),
        vcx.debug_bounds(title),
    ) else {
        return Err(format!(
            "pane {pane}: the card, its avatar and its name must all be drawn"
        ));
    };
    Ok(CardPieces {
        card,
        icon,
        title,
        marker: vcx.debug_bounds(marker),
    })
}

/// Width of `text` shaped in the theme font at `rems` of the window's rem size.
fn shaped_width(vcx: &mut VisualTestContext, text: &str, rems: f32) -> f32 {
    use gpui_component::ActiveTheme as _;
    vcx.update(|window, cx| {
        let font = gpui::Font {
            family: cx.theme().font_family.clone(),
            features: Default::default(),
            fallbacks: None,
            weight: Default::default(),
            style: Default::default(),
        };
        crate::ui::tab_strip::measure_text(
            window.text_system(),
            &font,
            rems * window.rem_size().as_f32(),
            text,
        )
    })
}

fn overlaps(a: &Bounds, b: &Bounds) -> bool {
    let slack = gpui::px(0.5);
    a.left() < b.right() - slack
        && b.left() < a.right() - slack
        && a.top() < b.bottom() - slack
        && b.top() < a.bottom() - slack
}

/// How wide the card is in the case under test.
#[derive(Clone, Copy)]
enum CardWidth {
    /// The board's floor: every column is exactly 210px.
    Narrowest,
    /// Room to spare: at least 300px.
    Roomy,
}

/// Draws the board for the split's Claude Code (pane 1) and Codex (pane 9)
/// cards in `locale` at `ui_size` and checks both cards' name and marker.
/// `beside` is `Some(true)` when the marker must stay on the name's line.
fn card_name_and_marker(
    cx: &mut TestAppContext,
    locale: &str,
    ui_size: f32,
    width: CardWidth,
    beside: Option<bool>,
) {
    let _locale = Locale::set(locale);
    let (app, mut vcx, _streams) = mixed_split_at(cx, Some(ui_size));
    let window_width = match width {
        CardWidth::Narrowest => 900.,
        CardWidth::Roomy => 2600.,
    };
    vcx.simulate_resize(gpui::size(gpui::px(window_width), gpui::px(900.)));
    show(&app, &mut vcx, ViewMode::Kanban, None);
    draw_frames(&mut vcx);
    let mut failures = Vec::new();
    for (pane, name) in [(1u64, claude()), (9, codex())] {
        let marker_text = app.read_with(&vcx, |app, cx| {
            app.agent_pane_summaries(cx)
                .iter()
                .find(|p| p.location.key.pane == pane)
                .and_then(|p| p.pane_marker())
                .unwrap_or_else(|| panic!("pane {pane} has no marker"))
        });
        let pieces = match card_pieces(&mut vcx, pane) {
            Ok(pieces) => pieces,
            Err(error) => {
                failures.push(error);
                continue;
            }
        };
        let CardPieces {
            card,
            icon,
            title,
            marker,
        } = pieces;
        let card_w = card.size.width.as_f32();
        fail_if(
            &mut failures,
            match width {
                CardWidth::Narrowest => (card_w - 210.).abs() > 0.5,
                CardWidth::Roomy => card_w < 300.,
            },
            || format!("setup: pane {pane}'s card is {card_w}px wide, not the case's width"),
        );
        let Some(marker) = marker else {
            failures.push(format!(
                "pane {pane}: the marker {marker_text:?} is not drawn"
            ));
            continue;
        };
        // The card's own edge-to-content inset, as drawn (border + padding).
        let inset = icon.left() - card.left();
        let content_right = card.right() - inset;
        let available = (content_right - title.left()).as_f32();
        let name_w = shaped_width(&mut vcx, name, 0.875);
        let marker_w = shaped_width(&mut vcx, &marker_text, 0.75);

        // The name keeps the width its text needs, up to what the card has.
        let want = name_w.min(available);
        fail_if(
            &mut failures,
            title.size.width.as_f32() < want - 0.5,
            || {
                format!(
                    "pane {pane}: name {name:?} is {}px wide, its text needs {name_w}px \
                     (card offers {available}px): the marker took its width",
                    title.size.width.as_f32()
                )
            },
        );
        fail_if(
            &mut failures,
            title.right() > content_right + gpui::px(0.5),
            || format!("pane {pane}: name {title:?} leaves the card {card:?}"),
        );

        // The marker is whole, inside the card, right of the avatar and
        // clear of the name.
        fail_if(
            &mut failures,
            marker.size.width.as_f32() < marker_w.min(available) - 0.5,
            || {
                format!(
                    "pane {pane}: marker {marker_text:?} is {}px wide, its text needs {marker_w}px",
                    marker.size.width.as_f32()
                )
            },
        );
        fail_if(
            &mut failures,
            marker.right() > content_right + gpui::px(0.5)
                || marker.left() < icon.right() - gpui::px(0.5)
                || marker.top() < card.top()
                || marker.bottom() > card.bottom() + gpui::px(0.5),
            || format!("pane {pane}: marker {marker:?} is not inside the card {card:?}"),
        );
        fail_if(&mut failures, overlaps(&title, &marker), || {
            format!("pane {pane}: marker {marker:?} overlaps the name {title:?}")
        });

        // Beside the name or the line under it, nowhere else.
        let same_line = marker.top() < title.bottom() - gpui::px(0.5);
        if same_line {
            fail_if(
                &mut failures,
                marker.left() < title.right() - gpui::px(0.5),
                || format!("pane {pane}: marker {marker:?} is before the name {title:?}"),
            );
        } else {
            fail_if(
                &mut failures,
                marker.left() < title.left() - gpui::px(0.5),
                || {
                    format!(
                        "pane {pane}: wrapped marker {marker:?} starts left of the name {title:?}"
                    )
                },
            );
        }
        if let Some(want_beside) = beside {
            fail_if(&mut failures, same_line != want_beside, || {
                format!(
                    "pane {pane}: marker {marker:?} is {} the name {title:?}, want {}",
                    if same_line { "beside" } else { "under" },
                    if want_beside { "beside" } else { "under" }
                )
            });
        }
    }
    assert_no_failures(failures);
}

/// RED today: at 210px, UI24, ru the marker is `flex_none` and takes the line,
/// so the name is squeezed to `Co…` / `Cla…`. The name keeps its text's width
/// and the marker wraps under it whole.
#[gpui::test]
fn a51_narrow_card_ru_ui24_gives_the_name_its_width_and_the_marker_its_own_line(
    cx: &mut TestAppContext,
) {
    card_name_and_marker(cx, "ru", 24., CardWidth::Narrowest, Some(false));
}

/// Guard: with room to spare the marker stays beside the name, the name
/// unsqueezed, in English at UI16.
#[gpui::test]
fn a51_roomy_card_en_ui16_keeps_the_marker_beside_the_name(cx: &mut TestAppContext) {
    card_name_and_marker(cx, "en", 16., CardWidth::Roomy, Some(true));
}

/// Guard: a name longer than the narrowest card stays on one line, inside the
/// card, while its text overflows the room (so it is cut, not wrapped or
/// pushed out). This proves the bounds, the single line and the natural
/// overflow; it does NOT observe the painted ellipsis glyph, which plain
/// `truncate` exposes no probe for. A custom label names a single-pane tab (a
/// split pane is always titled by its agent), so this card has no marker.
#[gpui::test]
fn a51_narrow_card_long_custom_label_stays_single_line_and_contained(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, mut streams) = mixed_split_at(cx, Some(24.));
    let long = "Review the authentication migration plan before the Friday release";
    report(
        &mut streams[1],
        CLIAgent::Codex,
        AgentStatus::Waiting,
        Some("Approve task"),
        false,
    );
    wait_for(&app, &mut vcx, "pane 2 waiting", |app, cx| {
        app.agent_pane_summaries(cx)
            .iter()
            .any(|p| p.location.key.pane == 2 && p.state == State::Waiting)
    });
    app.update_in(&mut vcx, |app, _, _| {
        app.tabs[1].name = Some(long.into());
    });
    assert_eq!(title_of(&app, &vcx, 2), long);
    vcx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(900.)));
    show(&app, &mut vcx, ViewMode::Kanban, None);
    draw_frames(&mut vcx);
    let mut failures = Vec::new();
    let (long_card, short_card) = match (card_pieces(&mut vcx, 2), card_pieces(&mut vcx, 9)) {
        (Ok(long_card), Ok(short_card)) => (long_card, short_card),
        (long_card, short_card) => {
            panic!(
                "both cards must be drawn: {:?} {:?}",
                long_card.err(),
                short_card.err()
            )
        }
    };
    let inset = long_card.icon.left() - long_card.card.left();
    let content_right = long_card.card.right() - inset;
    let available = (content_right - long_card.title.left()).as_f32();
    let natural = shaped_width(&mut vcx, long, 0.875);
    fail_if(
        &mut failures,
        (long_card.card.size.width.as_f32() - 210.).abs() > 0.5,
        || {
            format!(
                "setup: the card is {}px wide, not 210",
                long_card.card.size.width.as_f32()
            )
        },
    );
    fail_if(&mut failures, natural <= available, || {
        format!("setup: the label ({natural}px) fits the card's {available}px")
    });
    fail_if(
        &mut failures,
        long_card.title.right() > content_right + gpui::px(0.5),
        || {
            format!(
                "the label {:?} leaves the card {:?}",
                long_card.title, long_card.card
            )
        },
    );
    fail_if(
        &mut failures,
        (long_card.title.size.height - short_card.title.size.height)
            .as_f32()
            .abs()
            > 0.5,
        || {
            format!(
                "the label wrapped: it is {:?} high, a one-line name is {:?}",
                long_card.title.size.height, short_card.title.size.height
            )
        },
    );
    fail_if(&mut failures, long_card.marker.is_some(), || {
        "a single-pane tab has no marker".into()
    });
    assert_no_failures(failures);
}

/// Control: an exited pane of a split keeps its agent's identity with the
/// age on its reason line.
#[gpui::test]
fn a51_exited_split_pane_keeps_its_agent_identity_and_age(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, mut streams) = mixed_split(cx);
    report(
        &mut streams[3],
        CLIAgent::Codex,
        AgentStatus::Idle,
        None,
        true,
    );
    wait_for(&app, &mut vcx, "pane 9 exited", |app, cx| {
        app.agent_pane_summaries(cx)
            .iter()
            .any(|p| p.location.key.pane == 9 && p.state == State::Exited)
    });
    assert_eq!(title_of(&app, &vcx, 9), codex());
    show(&app, &mut vcx, ViewMode::Inbox, None);
    let rows = rendered_rows(&mut vcx);
    let row = row_of(&rows, false, 9);
    // Exactly two supported formats: today's heading that joins the pane
    // marker, and the bare agent name once the marker is its own span (that
    // split is pinned by a51_split_marker_is_a_separate_span_...).
    let joined = format!("{} · pane 3/3", codex());
    assert!(
        row.title == codex() || row.title == joined,
        "exited pane 9 drawn title {:?}, want {:?} or {joined:?}",
        row.title,
        codex()
    );
    assert_eq!(row.reason, "Exited · 3m");
}

// ----- F3: meta and preview ---------------------------------------------------

/// When the displayed title already is the agent's name the meta line leaves
/// it out and keeps the project and age.
#[gpui::test]
fn a51_meta_omits_the_agent_name_the_title_already_supplies(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = mixed_split(cx);
    let age = crate::ui::home::compact_age(60);
    let mut failures = Vec::new();
    for mode in [ViewMode::Inbox, ViewMode::Kanban] {
        show(&app, &mut vcx, mode, None);
        let rows = rendered_rows(&mut vcx);
        let card = mode == ViewMode::Kanban;
        // Pane 2 is a single pane titled by its agent; pane 9 is a split pane.
        for pane in [2, 9] {
            let project = project_of(&app, &vcx, pane);
            let want = Some(format!("{project} · {age}"));
            let got = line_of(&rows, card, pane, AgentLine::Meta);
            fail_if(&mut failures, got != want, || {
                format!("{mode:?}: pane {pane} meta {got:?}, want {want:?}")
            });
        }
    }
    assert_no_failures(failures);
}

/// Control: a raw kind with no known agent keeps naming itself in the meta,
/// because the row's title does not.
#[gpui::test]
fn a51_unknown_agent_kind_keeps_its_name_in_the_meta(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    report_kind(
        &mut streams[0],
        None,
        "devin",
        AgentStatus::Working,
        None,
        false,
    );
    wait_for(&app, &mut vcx, "the unknown agent", |app, cx| {
        app.agent_pane_summaries(cx).len() == 1
    });
    show(&app, &mut vcx, ViewMode::Inbox, None);
    let rows = rendered_rows(&mut vcx);
    let meta = line_of(&rows, false, 1, AgentLine::Meta);
    assert!(
        meta.as_deref()
            .is_some_and(|meta| meta.starts_with("devin · ")),
        "meta {meta:?} keeps the raw kind"
    );
}

/// An exited row's title is its agent, so its meta is the project alone; the
/// state and age stay on the reason line.
#[gpui::test]
fn a51_exited_meta_omits_the_agent_name(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    report(
        &mut streams[0],
        CLIAgent::Codex,
        AgentStatus::Idle,
        None,
        true,
    );
    wait_for(&app, &mut vcx, "the exit", |app, cx| {
        app.agent_pane_summaries(cx)
            .iter()
            .any(|p| p.state == State::Exited)
    });
    show(&app, &mut vcx, ViewMode::Inbox, None);
    let rows = rendered_rows(&mut vcx);
    let project = project_of(&app, &vcx, 1);
    assert_eq!(line_of(&rows, false, 1, AgentLine::Meta), Some(project));
}

/// Control: Idle and Exited keep their state word and age on every surface.
#[gpui::test]
fn a51_idle_and_exited_keep_state_and_age(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 2);
    report(
        &mut streams[0],
        CLIAgent::Codex,
        AgentStatus::Idle,
        None,
        false,
    );
    report(
        &mut streams[1],
        CLIAgent::Claude,
        AgentStatus::Idle,
        None,
        true,
    );
    wait_for(&app, &mut vcx, "idle and exited", |app, cx| {
        let panes = app.agent_pane_summaries(cx);
        panes.len() == 2 && panes.iter().any(|p| p.state == State::Exited)
    });
    let mut failures = Vec::new();
    for (mode, card) in [(ViewMode::Inbox, false), (ViewMode::Kanban, true)] {
        show(&app, &mut vcx, mode, None);
        let rows = rendered_rows(&mut vcx);
        for (pane, want) in [(1, "Idle"), (2, "Exited · 3m")] {
            let got = row_of(&rows, card, pane).reason;
            fail_if(&mut failures, got != want, || {
                format!("{mode:?}: pane {pane} reason {got:?}, want {want:?}")
            });
        }
    }
    for (selector, what) in [
        ("agent-row-reason-0-1", "Idle reason"),
        ("agent-row-reason-0-2", "Exited reason"),
    ] {
        fail_if(&mut failures, vcx.debug_bounds(selector).is_none(), || {
            format!("the {what} line is not drawn")
        });
    }
    assert_no_failures(failures);
}

fn reason_selector(pane: u64) -> &'static str {
    match pane {
        1 => "agent-row-reason-0-1",
        2 => "agent-row-reason-0-2",
        3 => "agent-row-reason-0-3",
        4 => "agent-row-reason-0-4",
        5 => "agent-row-reason-0-5",
        6 => "agent-row-reason-0-6",
        _ => panic!("no reason selector for pane {pane}"),
    }
}

/// Six agents with no message of their own except pane 6's question:
/// 1 Working, 2 Waiting, 3 Done, 4 Idle, 5 Exited, 6 Waiting "Allow Bash: pnpm test?".
fn six_states(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext, Vec<Stream>) {
    let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 6);
    for (stream, (status, message, exited)) in streams.iter_mut().zip([
        (AgentStatus::Working, None, false),
        (AgentStatus::Waiting, None, false),
        (AgentStatus::Done, None, false),
        (AgentStatus::Idle, None, false),
        (AgentStatus::Idle, None, true),
        (AgentStatus::Waiting, Some("Allow Bash: pnpm test?"), false),
    ]) {
        report(stream, CLIAgent::Codex, status, message, exited);
    }
    wait_for(&app, &mut vcx, "six agent states", |app, cx| {
        let panes = app.agent_pane_summaries(cx);
        panes.len() == 6 && panes.iter().any(|p| p.state == State::Exited)
    });
    (app, vcx, streams)
}

/// On a board card whose column already says Working, Waiting or Done, the
/// bare state word is not repeated; Idle and Exited keep theirs, and a real
/// question is never hidden.
#[gpui::test]
fn a51_kanban_hides_only_a_bare_state_word_its_column_supplies(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = six_states(cx);
    show(&app, &mut vcx, ViewMode::Kanban, None);
    let _ = rendered_rows(&mut vcx);
    let mut failures = Vec::new();
    for (pane, drawn, why) in [
        (1, false, "Working card in the Working column"),
        (2, false, "Waiting card in the Waiting column"),
        (3, false, "Done card in the Done column"),
        (4, true, "Idle keeps its state word"),
        (5, true, "Exited keeps its state and age"),
        (6, true, "a real question is never hidden"),
    ] {
        let got = vcx.debug_bounds(reason_selector(pane)).is_some();
        fail_if(&mut failures, got != drawn, || {
            format!("pane {pane} ({why}): reason line drawn = {got}, want {drawn}")
        });
    }
    assert_no_failures(failures);
}

/// Control: the question itself is the reason text on the card and the row.
#[gpui::test]
fn a51_waiting_question_stays_the_reason(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = six_states(cx);
    for (mode, card) in [(ViewMode::Inbox, false), (ViewMode::Kanban, true)] {
        show(&app, &mut vcx, mode, None);
        let rows = rendered_rows(&mut vcx);
        assert_eq!(
            row_of(&rows, card, 6).reason,
            "Allow Bash: pnpm test?",
            "{mode:?}"
        );
    }
}

/// In the Inbox the Other group supplies a Working agent's state, so its bare
/// word is dropped there; the Needs-you group says nothing of state, so a bare
/// Waiting keeps its word.
#[gpui::test]
fn a51_inbox_hides_a_bare_state_word_only_in_the_other_group(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = six_states(cx);
    show(&app, &mut vcx, ViewMode::Inbox, None);
    let _ = rendered_rows(&mut vcx);
    let mut failures = Vec::new();
    for (pane, drawn, why) in [
        (1, false, "Working row in the Other group"),
        (2, true, "bare Waiting in Needs you keeps its word"),
        (4, true, "Idle keeps its state word"),
        (5, true, "Exited keeps its state and age"),
    ] {
        let got = vcx.debug_bounds(reason_selector(pane)).is_some();
        fail_if(&mut failures, got != drawn, || {
            format!("pane {pane} ({why}): reason line drawn = {got}, want {drawn}")
        });
    }
    assert_no_failures(failures);
}

/// One Waiting agent whose terminal shows `output` under its `message`; the
/// board's card for it: the preview text its element was handed (`None` when
/// the card built none) and whether a preview element was laid out at all.
fn kanban_card(cx: &mut TestAppContext, message: &str, output: &str) -> (Option<String>, bool) {
    let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    report(
        &mut streams[0],
        CLIAgent::Codex,
        AgentStatus::Waiting,
        Some(message),
        false,
    );
    DaemonMsg::Output(format!("{output}\r\n").into_bytes())
        .encode(&mut streams[0])
        .unwrap();
    wait_for(&app, &mut vcx, "the preview line", |app, cx| {
        app.agent_pane_summaries(cx)
            .first()
            .and_then(|p| p.last_line.as_deref())
            == Some(output.trim())
    });
    show(&app, &mut vcx, ViewMode::Kanban, None);
    let rows = rendered_rows(&mut vcx);
    let _ = row_of(&rows, true, 1);
    let text = line_of(&rows, true, 1, AgentLine::Preview);
    let drawn = vcx.debug_bounds("agent-row-preview-0-1").is_some();
    (text, drawn)
}

const QUESTION: &str = "Allow Bash: pnpm test";

/// A preview that is the reason, character for character, says nothing new.
#[gpui::test]
fn a51_preview_equal_to_the_reason_is_not_repeated(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (text, drawn) = kanban_card(cx, QUESTION, QUESTION);
    assert!(
        !drawn && text.is_none(),
        "the preview line is drawn: {text:?}"
    );
}

/// Equal up to the whitespace the terminal laid out is still equal.
#[gpui::test]
fn a51_preview_equal_up_to_whitespace_is_not_repeated(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (text, drawn) = kanban_card(cx, QUESTION, "  Allow  Bash:   pnpm test ");
    assert!(
        !drawn && text.is_none(),
        "the preview line is drawn: {text:?}"
    );
}

/// Control: a longer line that continues the question carries new
/// information and stays whole.
#[gpui::test]
fn a51_preview_that_continues_the_reason_is_kept(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let output = "Allow Bash: pnpm test --filter checkout? (y/n)";
    let (text, drawn) = kanban_card(cx, QUESTION, output);
    assert!(drawn, "the continuation is not drawn: {text:?}");
    assert_eq!(text.as_deref(), Some(output));
}

/// Control: short unrelated output that happens to be a fragment of the
/// question is not erased.
#[gpui::test]
fn a51_unrelated_short_preview_inside_the_reason_is_kept(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (text, drawn) = kanban_card(cx, QUESTION, "pnpm");
    assert!(drawn, "the short output is not drawn: {text:?}");
    assert_eq!(text.as_deref(), Some("pnpm"));
}

/// Control: unrelated output is the preview's whole point.
#[gpui::test]
fn a51_distinct_preview_is_kept(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (text, drawn) = kanban_card(cx, QUESTION, "Running auth tests");
    assert!(drawn, "the preview is not drawn: {text:?}");
    assert_eq!(text.as_deref(), Some("Running auth tests"));
}

// ----- F7: edges ----------------------------------------------------------------

/// The Inbox/board fixture at a given UI size: two agents, both in `status`.
fn agents_ui(
    cx: &mut TestAppContext,
    status: AgentStatus,
    ui_size: f32,
) -> (Entity<Tty59App>, VisualTestContext, Vec<Stream>) {
    let (app, mut vcx, mut streams) =
        crate::ui::app::test_window::harness_with_tabs_configured(cx, 2, |config| {
            config.ui_font_size = ui_size;
        });
    for stream in streams.iter_mut().take(2) {
        report(stream, CLIAgent::Codex, status, Some("Approve task"), false);
    }
    wait_for(&app, &mut vcx, "two agents", |app, cx| {
        app.agent_pane_summaries(cx).len() == 2
    });
    (app, vcx, streams)
}

fn projects_label_edge(cx: &mut TestAppContext, ui_size: f32) {
    let (app, mut vcx, _streams) = agents_ui(cx, AgentStatus::Waiting, ui_size);
    show(&app, &mut vcx, ViewMode::Inbox, Some(1));
    draw_frames(&mut vcx);
    let label = vcx.debug_bounds("agent-projects-label");
    let name = vcx.debug_bounds("agent-project-name-0");
    let (Some(label), Some(name)) = (label, name) else {
        panic!("Projects label {label:?} and the first project's name {name:?} must be drawn");
    };
    assert!(
        near(label.left(), name.left()),
        "UI{ui_size}: Projects starts at {:?} but project names at {:?}",
        label.left(),
        name.left()
    );
}

/// "Projects" starts where the project names under it do, at every UI size.
#[gpui::test]
fn a51_projects_label_shares_the_project_name_edge_at_ui16(cx: &mut TestAppContext) {
    projects_label_edge(cx, 16.);
}

#[gpui::test]
fn a51_projects_label_shares_the_project_name_edge_at_ui24(cx: &mut TestAppContext) {
    projects_label_edge(cx, 24.);
}

fn inbox_edges(cx: &mut TestAppContext, ui_size: f32) {
    let (app, mut vcx, _streams) = agents_ui(cx, AgentStatus::Waiting, ui_size);
    show(&app, &mut vcx, ViewMode::Inbox, Some(1));
    draw_frames(&mut vcx);
    let mut failures = Vec::new();
    let heading = vcx.debug_bounds("agent-inbox-heading");
    let needs = vcx.debug_bounds("agent-needs-you");
    let avatar = vcx.debug_bounds("agent-row-icon-0-1");
    let list = vcx.debug_bounds("agent-inbox-list");
    let (Some(heading), Some(needs), Some(avatar), Some(list)) = (heading, needs, avatar, list)
    else {
        panic!(
            "Inbox heading {heading:?}, Needs you {needs:?}, avatar {avatar:?} and list {list:?} must all be drawn"
        );
    };
    fail_if(&mut failures, !near(heading.left(), needs.left()), || {
        format!(
            "UI{ui_size}: Inbox heading at {:?}, Needs you at {:?}",
            heading.left() - list.left(),
            needs.left() - list.left()
        )
    });
    fail_if(&mut failures, !near(avatar.left(), needs.left()), || {
        format!(
            "UI{ui_size}: avatar at {:?}, Needs you at {:?}",
            avatar.left() - list.left(),
            needs.left() - list.left()
        )
    });
    assert_no_failures(failures);
}

/// The Inbox heading, "Needs you" and the avatars share one left edge.
#[gpui::test]
fn a51_inbox_heading_needs_you_and_avatars_share_an_edge_at_ui16(cx: &mut TestAppContext) {
    inbox_edges(cx, 16.);
}

#[gpui::test]
fn a51_inbox_heading_needs_you_and_avatars_share_an_edge_at_ui24(cx: &mut TestAppContext) {
    inbox_edges(cx, 24.);
}

fn board_edges(cx: &mut TestAppContext, ui_size: f32) {
    let (app, mut vcx, _streams) = agents_ui(cx, AgentStatus::Waiting, ui_size);
    show(&app, &mut vcx, ViewMode::Kanban, None);
    draw_frames(&mut vcx);
    let heading = vcx.debug_bounds("agent-board-heading-0");
    let avatar = vcx.debug_bounds("agent-row-icon-0-1");
    let (Some(heading), Some(avatar)) = (heading, avatar) else {
        panic!(
            "the Waiting column heading {heading:?} and its card's avatar {avatar:?} must be drawn"
        );
    };
    assert!(
        near(heading.left(), avatar.left()),
        "UI{ui_size}: column heading at {:?}, card avatar at {:?}",
        heading.left(),
        avatar.left()
    );
}

/// A board column's title starts where its cards' avatars do.
#[gpui::test]
fn a51_board_heading_shares_the_card_avatar_edge_at_ui16(cx: &mut TestAppContext) {
    board_edges(cx, 16.);
}

#[gpui::test]
fn a51_board_heading_shares_the_card_avatar_edge_at_ui24(cx: &mut TestAppContext) {
    board_edges(cx, 24.);
}

// ----- F9: Other agents ----------------------------------------------------------

/// The pieces of the Other-agents control, drawn with `ui_size` in `locale`.
fn other_toggle_geometry(cx: &mut TestAppContext, ui_size: f32, locale: &str, height: Option<f32>) {
    let _locale = Locale::set(locale);
    let (app, mut vcx, _streams) = agents_ui(cx, AgentStatus::Working, ui_size);
    show(&app, &mut vcx, ViewMode::Inbox, None);
    app.update_in(&mut vcx, |app, _, _| {
        app.agent_vision.state.show_other = false;
    });
    draw_frames(&mut vcx);
    let mut failures = Vec::new();
    let toggle = vcx.debug_bounds("agent-other-toggle");
    let needs = vcx.debug_bounds("agent-needs-you");
    let label = vcx.debug_bounds("agent-other-toggle-label");
    let count = vcx.debug_bounds("agent-other-toggle-count");
    let chevron = vcx.debug_bounds("agent-other-chevron-collapsed");
    let (Some(toggle), Some(needs)) = (toggle, needs) else {
        panic!("the control {toggle:?} and Needs you {needs:?} must be drawn");
    };
    if let Some(height) = height {
        fail_if(
            &mut failures,
            (toggle.size.height.as_f32() - height).abs() > 0.01,
            || format!("control is {:?} high, want {height}", toggle.size.height),
        );
    } else {
        fail_if(
            &mut failures,
            toggle.size.height.as_f32() < 28. - 0.01,
            || format!("control is {:?} high, want at least 28", toggle.size.height),
        );
    }
    for (name, part) in [("label", label), ("count", count), ("chevron", chevron)] {
        fail_if(&mut failures, part.is_none(), || {
            format!("the control draws no separate {name}")
        });
    }
    if let (Some(label), Some(count), Some(chevron)) = (label, count, chevron) {
        fail_if(&mut failures, !near(label.left(), needs.left()), || {
            format!(
                "label starts at {:?}, Needs you at {:?}",
                label.left(),
                needs.left()
            )
        });
        fail_if(
            &mut failures,
            count.left() < label.right() - gpui::px(0.5),
            || format!("count {count:?} overlaps the label {label:?}"),
        );
        fail_if(
            &mut failures,
            chevron.left() < count.right() - gpui::px(0.5),
            || format!("chevron {chevron:?} overlaps the count {count:?}"),
        );
        fail_if(
            &mut failures,
            chevron.right() > toggle.right() + gpui::px(0.5)
                || label.left() < toggle.left() - gpui::px(0.5),
            || format!("the pieces leave the control {toggle:?}"),
        );
        for (name, part) in [("label", label), ("count", count), ("chevron", chevron)] {
            fail_if(
                &mut failures,
                (part.center().y - toggle.center().y).as_f32().abs() > 1.,
                || format!("{name} {part:?} is not centred in {toggle:?}"),
            );
        }
    }
    assert_no_failures(failures);
}

/// Left-aligned 28px section control, its label, count and chevron separate
/// and inside it.
#[gpui::test]
fn a51_other_toggle_geometry_ui16_en(cx: &mut TestAppContext) {
    other_toggle_geometry(cx, 16., "en", Some(28.));
}

/// The longest locale at the largest UI size still fits the pieces in the
/// control without overlap.
#[gpui::test]
fn a51_other_toggle_geometry_ui24_ru(cx: &mut TestAppContext) {
    other_toggle_geometry(cx, 24., "ru", None);
}

/// The chevron says collapsed or expanded: closed it points at the label,
/// open it points down, and the state follows the real toggle.
#[gpui::test]
fn a51_other_toggle_chevron_follows_the_open_state(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = agents_ui(cx, AgentStatus::Working, 16.);
    show(&app, &mut vcx, ViewMode::Inbox, None);
    let mut failures = Vec::new();
    for (open, drawn, absent) in [
        (
            false,
            "agent-other-chevron-collapsed",
            "agent-other-chevron-expanded",
        ),
        (
            true,
            "agent-other-chevron-expanded",
            "agent-other-chevron-collapsed",
        ),
    ] {
        app.update_in(&mut vcx, |app, _, _| {
            app.agent_vision.state.show_other = open;
        });
        draw_frames(&mut vcx);
        fail_if(&mut failures, vcx.debug_bounds(drawn).is_none(), || {
            format!("open = {open}: {drawn} is not drawn")
        });
        fail_if(&mut failures, vcx.debug_bounds(absent).is_some(), || {
            format!("open = {open}: {absent} is drawn")
        });
    }
    assert_no_failures(failures);
}

/// Control: a click on the control opens and closes the Other group through
/// the existing callback, and the list takes focus.
#[gpui::test]
fn a51_other_toggle_click_flips_the_group(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = agents_ui(cx, AgentStatus::Working, 16.);
    show(&app, &mut vcx, ViewMode::Inbox, None);
    app.update_in(&mut vcx, |app, _, _| {
        app.agent_vision.state.show_other = false;
    });
    draw_frames(&mut vcx);
    assert!(vcx.debug_bounds("agent-row-frame-0-1").is_none());
    for (open, rows) in [(true, true), (false, false)] {
        let toggle = vcx
            .debug_bounds("agent-other-toggle")
            .expect("the control is drawn");
        vcx.simulate_click(toggle.center(), gpui::Modifiers::none());
        draw_frames(&mut vcx);
        assert_eq!(other_open(&app, &vcx), open, "after the click");
        assert_eq!(
            vcx.debug_bounds("agent-row-frame-0-1").is_some(),
            rows,
            "the Other rows follow the state"
        );
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(
                app.agent_vision.list_focus.contains_focused(window, cx),
                "the list owns focus after the toggle"
            );
        });
    }
}

/// A clean starting point for one keyboard attempt: the Inbox, Other closed,
/// an empty query, focus on the list, then `steps` Tab stops forward, painted.
/// (The A5 variant of `tab_stop_attempt`, painting through `draw_frames`; the
/// `press` and `other_open` helpers above are shared.)
fn space_tab_stop_attempt(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, steps: usize) {
    app.update_in(vcx, |app, window, cx| {
        app.set_agent_mode(ViewMode::Inbox, window, cx);
        app.agent_vision.state.show_other = false;
        let input = app.agent_vision.input.clone();
        input.update(cx, |input, cx| input.set_value("", window, cx));
    });
    draw_frames(vcx);
    app.update_in(vcx, |app, window, cx| {
        window.focus(&app.agent_vision.list_focus, cx);
        for _ in 0..steps {
            window.focus_next(cx);
        }
    });
    // Painted, so the focused element knows it is focused when the key comes up.
    draw_frames(vcx);
}

/// The number of Tab stops from the list, counted from the closed state, at
/// which Space opens the Other group, or `None` when no stop within the first
/// 24 does. Only Space discovers it: an Enter pressed on an arbitrary earlier
/// control can do anything that control does.
fn space_toggle_tab_stop(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Option<usize> {
    (1..=24).find(|&steps| {
        space_tab_stop_attempt(app, vcx, steps);
        press(vcx, "space");
        other_open(app, vcx)
    })
}

/// The control is a Tab stop that Space activates.
#[gpui::test]
fn a51_other_toggle_space_opens_the_group_from_a_tab_stop(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = agents_ui(cx, AgentStatus::Working, 16.);
    show(&app, &mut vcx, ViewMode::Inbox, None);
    assert!(
        space_toggle_tab_stop(&app, &mut vcx).is_some(),
        "Space on no Tab stop within 24 opened the Other group"
    );
}

/// The keyboard runs the same callback as the click, in both directions: the
/// control's own focus handle (the one focused when Space opened the group) is
/// kept, and focusing that same control again lets Space close it. Each press
/// hands focus to the list and the Other rows follow the state. (Enter on this
/// control belongs to the keyboard-navigation change and is not asserted here.)
#[gpui::test]
fn a51_other_toggle_space_matches_the_click_callback(cx: &mut TestAppContext) {
    let _locale = Locale::set("en");
    let (app, mut vcx, _streams) = agents_ui(cx, AgentStatus::Working, 16.);
    show(&app, &mut vcx, ViewMode::Inbox, None);
    let steps = space_toggle_tab_stop(&app, &mut vcx).expect("Space finds the control's Tab stop");
    space_tab_stop_attempt(&app, &mut vcx, steps);
    let control = vcx
        .update(|window, cx| window.focused(cx))
        .expect("a control is focused at the Tab stop");
    assert!(
        vcx.debug_bounds("agent-row-frame-0-1").is_none(),
        "the Other rows start hidden"
    );
    press(&mut vcx, "space");
    draw_frames(&mut vcx);
    assert!(other_open(&app, &vcx), "Space opens");
    assert!(
        vcx.debug_bounds("agent-row-frame-0-1").is_some(),
        "the Other rows appear once it is open"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(
            app.agent_vision.list_focus.contains_focused(window, cx),
            "the list owns focus after Space, as after a click"
        );
        window.focus(&control, cx);
    });
    draw_frames(&mut vcx);
    press(&mut vcx, "space");
    draw_frames(&mut vcx);
    assert!(!other_open(&app, &vcx), "Space on the same control closes");
    assert!(
        vcx.debug_bounds("agent-row-frame-0-1").is_none(),
        "the Other rows disappear again"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(
            app.agent_vision.list_focus.contains_focused(window, cx),
            "the list owns focus after the closing Space, as after a click"
        );
    });
}
