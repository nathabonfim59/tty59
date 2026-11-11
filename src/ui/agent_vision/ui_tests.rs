use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};

use super::{FocusZone, ViewMode};
use crate::core::cli_agent::{AgentDetectionSession, AgentSessionState, AgentStatus};
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
