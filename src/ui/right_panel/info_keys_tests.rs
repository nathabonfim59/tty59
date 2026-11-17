//! POST-IMPLEMENTATION controls for the Info rows' keyboard that need the
//! module's private state, written AFTER `info_row_key` and the entry in
//! `right_panel.rs`. They are not RED-first. The behavioural ones through real
//! keys and the clipboard are in `scm/rows_info_tests.rs`; this file has:
//!
//! * the pure part: which act each key stands for on each kind of row, and
//!   that Space is Reveal only;
//! * the entry's guards, seen through the real frame: a request made for one
//!   pane and answered by a frame that draws another (two real tabs) is
//!   dropped and never focuses a row, and nothing settles after the keyboard
//!   comes back to the first pane;
//! * the ticket rule: a completion for an older request leaves a newer entry
//!   where it is, and a matching one is consumed whatever the answer. The
//!   older completion is delivered by calling the method with a `Pending`
//!   built here (the frames cannot be made to deliver a stale one), which is
//!   labelled as such below.
//!
//! The request in the two-tab control is made by the same method the root's
//! Down key calls, in the same update that switches the tab, so the frame that
//! answers it is the other pane's.

use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::daemon::protocol::DaemonMsg;
use crate::ui::app::test_window;
use crate::ui::host_ops::HostId;

fn copy_row(label: &'static str, text: &str) -> InfoRow {
    InfoRow {
        label,
        value: InfoValue::Text(text.to_string()),
        copy: Some(text.to_string()),
        reveal: None,
    }
}

fn counts(open: bool, added: u32) -> InfoRow {
    InfoRow {
        label: "changes",
        value: InfoValue::Diff {
            added,
            removed: 0,
            open: open.then(|| (HostId::LOCAL, PathBuf::from("/w/repo"))),
        },
        copy: None,
        reveal: None,
    }
}

#[test]
fn enter_does_what_a_tile_does_and_space_is_reveal_only() {
    // A branch: Enter is the copy tile's text, Space nothing.
    let branch = copy_row("branch", "feature/x");
    assert!(matches!(branch.enter_act(), Some(InfoAct::Copy(t)) if t == "feature/x"));
    assert!(branch.space_act().is_none(), "Space is not Copy");

    // The working directory on this machine: both tiles; Enter is Copy with
    // the string the tile writes, Space is Reveal of the very path it holds.
    let cwd = InfoRow {
        label: "cwd",
        value: InfoValue::Path("~/repo".into()),
        copy: Some("/w/repo".into()),
        reveal: Some(PathBuf::from("/w/repo")),
    };
    assert!(matches!(cwd.enter_act(), Some(InfoAct::Copy(t)) if t == "/w/repo"));
    assert!(
        matches!(cwd.space_act(), Some(InfoAct::Reveal(p)) if p == std::path::Path::new("/w/repo"))
    );

    // A remote working directory has no Reveal: Space does nothing.
    let remote = InfoRow {
        reveal: None,
        ..InfoRow {
            label: "cwd",
            value: InfoValue::Path("/srv/app".into()),
            copy: Some("/srv/app".into()),
            reveal: None,
        }
    };
    assert!(remote.space_act().is_none());

    // The counts open their diff, and a clean tree or no target does nothing.
    assert!(matches!(
        counts(true, 3).enter_act(),
        Some(InfoAct::OpenDiff(host, p)) if host == HostId::LOCAL && p == PathBuf::from("/w/repo")
    ));
    assert!(counts(true, 3).space_act().is_none());
    assert!(counts(false, 3).enter_act().is_none());
    assert!(counts(false, 0).enter_act().is_none());
    // A readout has neither.
    let shell = InfoRow::text("shell", "zsh".into());
    assert!(shell.enter_act().is_none() && shell.space_act().is_none());
}

#[test]
fn the_context_key_values_are_the_frozen_names() {
    assert_eq!(InfoKey::Pane.as_str(), "Pane");
    assert_eq!(InfoKey::Cwd.as_str(), "Cwd");
    assert_eq!(InfoKey::Branch.as_str(), "Branch");
    assert_eq!(InfoKey::Changes.as_str(), "Changes");
}

// ---------------------------------------------------------------- harness --

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tty59-info-keys-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(&dir).unwrap()
}

fn draw(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |_, _, cx| cx.notify());
    vcx.run_until_parked();
}

/// The panes of the first `n` tabs, each in a directory of its own, with the
/// Info tab asked for by name (the keyboard on its root) on the first.
fn two_tabs(
    cx: &mut TestAppContext,
    name: &str,
) -> (
    gpui::Entity<Tty59App>,
    VisualTestContext,
    (u64, u64),
    Vec<PathBuf>,
) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx, mut streams) = test_window::harness_with_tabs(cx, 2);
    vcx.simulate_resize(gpui::size(px(1440.), px(900.)));
    let dirs = vec![dir(&format!("{name}-a")), dir(&format!("{name}-b"))];
    for (stream, path) in streams.iter_mut().zip(&dirs) {
        DaemonMsg::Cwd(path.clone())
            .encode(stream)
            .expect("the pane takes its directory");
    }
    app.update_in(&mut vcx, |app, window, cx| {
        app.right_panel_visible = true;
        app.show_right_panel_tab(RightPanelTab::Info, window, cx);
    });
    let mut panes = [0u64; 2];
    for (i, slot) in panes.iter_mut().enumerate() {
        app.update_in(&mut vcx, |app, _, cx| {
            app.active = i;
            cx.notify();
        });
        let mut found = None;
        for _ in 0..3000 {
            draw(&app, &mut vcx);
            found = app.update_in(&mut vcx, |app, window, cx| {
                let rows = app.info_rows(window, cx);
                rows.rows
                    .iter()
                    .any(|(key, _)| *key == InfoKey::Cwd)
                    .then_some(rows.pane)
                    .flatten()
            });
            if found.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        *slot = found.expect("the pane's working directory row never appeared");
    }
    app.update_in(&mut vcx, |app, window, cx| {
        app.active = 0;
        app.show_right_panel_tab(RightPanelTab::Info, window, cx);
    });
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_ne!(panes[0], panes[1], "the two tabs are two panes");
    (app, vcx, (panes[0], panes[1]), dirs)
}

/// Whether the keyboard is on the stop or inside it.
fn stop_focused(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
    app.update_in(vcx, |app, window, cx| {
        app.info_kb.roving.handle.contains_focused(window, cx)
    })
}

fn root_focused(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
    app.update_in(vcx, |app, window, _| app.info_focus.is_focused(window))
}

/// A request made for the first pane and answered by a frame that draws the
/// second: the frame's row is another pane's, so the entry is dropped, no row
/// takes the keyboard, and the position the request chose is not kept. Coming
/// back to the first pane afterwards settles nothing: there is no request left
/// for that frame to find.
#[cfg(unix)]
#[gpui::test]
fn a_request_for_one_pane_is_dropped_by_the_frame_of_another(cx: &mut TestAppContext) {
    let (app, mut vcx, (first, second), dirs) = two_tabs(cx, "cross");
    assert!(
        root_focused(&app, &mut vcx),
        "Info asked for: the root holds it"
    );
    assert!(!stop_focused(&app, &mut vcx));

    // The root's Down, and the tab switch, in one update: the next frame draws
    // the second pane.
    app.update_in(&mut vcx, |app, window, cx| {
        app.info_request_entry(false, window, cx);
        let at = app.info_kb.at.expect("the request chose a row");
        assert_eq!(at.pane, first, "it chose a row of the pane it was made in");
        assert!(app.info_kb.entry.is_some());
        app.active = 1;
        cx.notify();
    });
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.info_rows(window, cx).pane, Some(second));
        assert!(
            app.info_kb.entry.is_none(),
            "the other pane's frame left the entry"
        );
        assert!(
            app.info_kb.at.is_none(),
            "the position the request chose was kept"
        );
    });
    assert!(
        !stop_focused(&app, &mut vcx),
        "a row of the other pane took the keyboard"
    );
    assert!(
        root_focused(&app, &mut vcx),
        "the keyboard moved off the root"
    );

    // A, B, A: back on the first pane, nothing settles later.
    app.update_in(&mut vcx, |app, _, cx| {
        app.active = 0;
        cx.notify();
    });
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.info_rows(window, cx).pane, Some(first));
        assert!(app.info_kb.entry.is_none());
    });
    assert!(
        !stop_focused(&app, &mut vcx),
        "a late request took the keyboard"
    );
    assert!(root_focused(&app, &mut vcx));
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// With no row to answer it (the stop tracks nothing in this frame) the root
/// carries the probe and the entry is dropped just the same. A request is made
/// and the choice it made is then emptied the way a vanished row would empty
/// it, with the handle left focused so no row tracks it; the frame consumes
/// the entry and moves no focus.
#[cfg(unix)]
#[gpui::test]
fn the_root_probe_consumes_an_entry_no_row_answers(cx: &mut TestAppContext) {
    let (app, mut vcx, (first, _second), dirs) = two_tabs(cx, "root-probe");
    app.update_in(&mut vcx, |app, window, cx| {
        app.info_request_entry(false, window, cx);
        // A choice that names no drawn row (another pane), while the stop is
        // focused: nothing tracks the handle in the next frame.
        let key = app.info_kb.at.expect("chosen").key;
        app.info_kb.at = Some(InfoSource {
            pane: first + 1000,
            key,
        });
        let handle = app.info_kb.roving.handle.clone();
        window.focus(&handle, cx);
        cx.notify();
    });
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, _, _| {
        assert!(app.info_kb.entry.is_none(), "the entry outlived the frame");
    });
    assert!(
        !stop_focused(&app, &mut vcx) || root_focused(&app, &mut vcx),
        "a row was given the keyboard by an entry that no row answered"
    );
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// LABELLED: the older completion is delivered by hand, not by a frame. A
/// completion for an older ticket leaves a newer entry in place; the matching
/// one consumes it on a failing validation too, and takes back the position it
/// had chosen; one more for a ticket nobody holds does nothing.
#[cfg(unix)]
#[gpui::test]
fn an_older_completion_leaves_a_newer_entry_and_a_matching_one_is_always_consumed(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, (first, _second), dirs) = two_tabs(cx, "ticket");
    app.update_in(&mut vcx, |app, window, cx| {
        let source = InfoSource {
            pane: first,
            key: InfoKey::Cwd,
        };
        let pending = |app: &Tty59App, ticket: u64, window: &Window| Pending {
            ticket,
            window: window.window_handle(),
            origin: Some(app.info_focus.clone()),
            kind: PendingKind::Entry,
        };
        let newer = app.info_kb.nav_gen + 10;
        app.info_kb.at = Some(source);
        app.info_kb.entry = Some(InfoEntry {
            ticket: newer,
            source,
        });

        let older = pending(app, newer - 1, window);
        app.info_entry_complete(older, Some(source), window, cx);
        assert!(
            app.info_kb
                .entry
                .as_ref()
                .is_some_and(|e| e.ticket == newer),
            "an older completion took a newer request's entry"
        );
        assert_eq!(app.info_kb.at, Some(source), "and its position");

        // The matching ticket fails validation (the generation is not it) and
        // is consumed all the same, with the position it had chosen.
        let matching = pending(app, newer, window);
        app.info_entry_complete(matching, Some(source), window, cx);
        assert!(
            app.info_kb.entry.is_none(),
            "a matching completion left its entry"
        );
        assert!(app.info_kb.at.is_none(), "and its position");
        assert!(
            !app.info_kb.roving.handle.contains_focused(window, cx),
            "a failed validation moved the keyboard"
        );

        // Nothing is waiting any more: another one for that ticket is a no-op.
        let again = pending(app, newer, window);
        app.info_entry_complete(again, Some(source), window, cx);
        assert!(app.info_kb.entry.is_none());
    });
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}
