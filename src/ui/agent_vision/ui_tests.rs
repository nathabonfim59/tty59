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
    assert_eq!(inbox_p3.reason, "Working");
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
    assert_eq!(card_p3.reason, "Working");
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
