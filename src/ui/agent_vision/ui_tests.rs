use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};

use super::render::{take_rendered_headers, take_rendered_rows};
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
    let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 3);
    for (index, stream) in streams.iter_mut().take(2).enumerate() {
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status: AgentStatus::Waiting,
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
    assert!(vcx.debug_bounds("agent-row-dot-0-1").is_none());

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
    assert!(vcx.debug_bounds("agent-row-dot-0-2").is_none());

    let inbox_p3 = inbox_rows
        .iter()
        .find(|row| !row.card && row.key.pane == 3)
        .expect("live claude row rendered in Inbox");
    assert_eq!(inbox_p3.title, CLIAgent::Claude.display_name());
    assert_eq!(inbox_p3.reason, "Working");
    assert!(vcx.debug_bounds("row-status-circle-0").is_some());
    assert!(vcx.debug_bounds("agent-row-dot-0-4").is_some());

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
    assert!(vcx.debug_bounds("agent-row-dot-0-1").is_none());

    let card_p2 = kanban_rows
        .iter()
        .find(|row| row.card && row.key.pane == 2)
        .expect("exited gemini card rendered in Kanban");
    assert_eq!(card_p2.title, CLIAgent::Gemini.display_name());
    assert_eq!(card_p2.reason, "Exited");
    assert!(vcx.debug_bounds("agent-row-preview-0-2").is_none());
    assert!(vcx.debug_bounds("agent-row-dot-0-2").is_none());

    let card_p3 = kanban_rows
        .iter()
        .find(|row| row.card && row.key.pane == 3)
        .expect("live claude card rendered in Kanban");
    assert_eq!(card_p3.title, CLIAgent::Claude.display_name());
    assert_eq!(card_p3.reason, "Working");
    assert!(vcx.debug_bounds("agent-row-preview-0-3").is_some());
}
