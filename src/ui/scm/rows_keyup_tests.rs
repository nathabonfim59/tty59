//! Reviewer tests for Space as the platform delivers it: a key-down AND a
//! key-up, kept apart from the frozen `rows_keys_tests`.
//!
//! WHY THIS EXISTS. The frozen tests press a key with
//! `VisualTestContext::simulate_keystrokes`, which delivers the KEY-DOWN ONLY
//! (their `press` helper is `simulate_keystrokes` followed by a draw; nothing
//! sends a key-up). GPUI turns Enter and Space on a focused element into a click
//! on the KEY-UP: an element that tracks the focus handle and has `on_click`
//! listeners (a directory row's fold, a file row's diff) gets a
//! `ClickEvent::Keyboard` when the key is released (div.rs: the `is_focused`
//! key-up listener). So a key-down-only test cannot see what the release does.
//!
//! These tests deliver the real events: `simulate_keystrokes` for the key-down
//! (as the platform's own, `is_held` false) or a `KeyDownEvent` with `is_held`
//! set for a repeat, then a real `gpui::KeyUpEvent` through
//! `VisualTestContext::simulate_event`, the same pair the forwards tests use.
//! No click is injected and no action is logged: the only things observed are
//! the app's own state (fold set, diff overlay, git, the PTY).
//!
//! Scope: only what Space must NOT do on release. That Space stages a file row
//! stays pinned by the frozen tests (key-down), and nothing here asserts it.

use gpui::{
    Entity, KeyDownEvent, KeyUpEvent, Keystroke, PlatformInput, TestAppContext, VisualTestContext,
};

use super::rows_window_tests::{
    big_repo, draw, flat_rows, git_stdout, only_group, panel_on, serial, status,
};
use crate::core::actions::ShowRightPanelChanges;
use crate::daemon::protocol::ClientMsg;
use crate::ui::app::Tty59App;
use crate::ui::scm::panel::group_diff_source;
use crate::ui::scm::state::ScmGroup;

type Stream = crate::daemon::transport::Stream;

/// The id of the Changes node that holds the keyboard (the `row` of the
/// `ScmRows` key context on the focus path), the contract the frozen tests use.
fn marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|window, _| {
        window
            .context_stack()
            .iter()
            .find(|c| c.contains("ScmRows"))
            .and_then(|c| c.get("row"))
            .map(|v| v.to_string())
    })
}

fn pane_input(pane: &mut Stream) -> Vec<u8> {
    pane.set_read_timeout(Some(std::time::Duration::from_millis(150)))
        .expect("the pane link takes a read timeout");
    let mut out = Vec::new();
    loop {
        match ClientMsg::read(pane) {
            Ok(ClientMsg::Input(bytes)) => out.extend_from_slice(&bytes),
            Ok(_) => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(e) => panic!("pane link failed: {e}"),
        }
    }
    out
}

/// Real entry: the keyboard action that opens the Changes list.
fn enter(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
    vcx.dispatch_action(ShowRightPanelChanges);
    draw(app, vcx);
    draw(app, vcx);
}

fn down(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    vcx.simulate_keystrokes("down");
    draw(app, vcx);
}

/// Space as delivered by the platform: the key-down, then the key-up.
fn space_down_up(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, held: bool) {
    let keystroke = Keystroke::parse("space").expect("a keystroke");
    if held {
        // A repeat: the key-down carries `is_held`, as the platform sets it.
        vcx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: keystroke.clone(),
                    is_held: true,
                    prefer_character_input: false,
                }),
                cx,
            );
        });
        vcx.run_until_parked();
    } else {
        vcx.simulate_keystrokes("space");
    }
    vcx.simulate_event(KeyUpEvent { keystroke });
    draw(app, vcx);
}

fn staged_dir_folds(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> usize {
    app.update_in(vcx, |app, _, _| {
        app.scm
            .folded_dirs
            .iter()
            .filter(|(g, _)| *g == ScmGroup::Staged)
            .count()
    })
}

/// How many paths the index differs from HEAD in: what staging would move.
fn cached_paths(root: &std::path::Path) -> usize {
    git_stdout(root, &["diff", "--cached", "--name-only"])
        .lines()
        .count()
}

fn diff_open(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, group: ScmGroup) -> bool {
    app.update_in(vcx, |app, _, _| {
        let repo = app.scm.repo.clone().expect("a repository is on screen");
        app.diff_overlay_focus(repo.host, &repo.root, &group_diff_source(group))
            .is_some()
    })
}

/// Space on a tree directory, key-down AND key-up: the key-up would fold or
/// open the directory through its click listener. It must not: a directory
/// ignores Space completely. Entered by the real action, walked to by Down.
#[gpui::test]
fn space_down_and_up_on_a_directory_changes_nothing(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("keyup-dir");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
    only_group(&app, &mut vcx, ScmGroup::Staged);
    enter(&app, &mut vcx);

    let mut dir = None;
    for _ in 0..6 {
        let now = marker(&mut vcx);
        assert!(
            now.is_some(),
            "no `ScmRows` key context is mounted at the focus"
        );
        if now
            .as_deref()
            .is_some_and(|m| m.starts_with("scm-dir-Staged-"))
        {
            dir = now;
            break;
        }
        down(&app, &mut vcx);
    }
    let dir = dir.expect("Down reached a Staged directory row");
    let entries = status(&app, &mut vcx).entries.len();
    let cached = cached_paths(&root);
    let _ = pane_input(&mut pane);

    for held in [false, true] {
        assert_eq!(staged_dir_folds(&app, &mut vcx), 0);
        space_down_up(&app, &mut vcx, held);
        assert_eq!(
            staged_dir_folds(&app, &mut vcx),
            0,
            "Space (held: {held}) folded or opened the directory"
        );
        assert_eq!(marker(&mut vcx).as_deref(), Some(dir.as_str()));
    }
    // Folded first (Left is the real fold key), then Space: it stays folded.
    vcx.simulate_keystrokes("left");
    draw(&app, &mut vcx);
    assert_eq!(staged_dir_folds(&app, &mut vcx), 1, "Left folded it");
    for held in [false, true] {
        space_down_up(&app, &mut vcx, held);
        assert_eq!(
            staged_dir_folds(&app, &mut vcx),
            1,
            "Space (held: {held}) opened a folded directory"
        );
        assert_eq!(marker(&mut vcx).as_deref(), Some(dir.as_str()));
    }
    vcx.run_until_parked();
    assert!(!vcx.has_pending_prompt(), "Space on a directory asked");
    assert_eq!(status(&app, &mut vcx).entries.len(), entries);
    assert_eq!(cached_paths(&root), cached, "Space on a directory staged");
    assert!(
        pane_input(&mut pane).is_empty(),
        "the keys were consumed by the directory row, not typed into the pane"
    );
    app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}

/// Space on a file row, key-down AND key-up (and a held key-down): the key-up
/// would open the file's diff through its click listener. It must not; what
/// Space does to a file (stage) is pinned by the frozen tests and is not asserted
/// here, and neither is which node the keyboard ends on (staging moves it to a
/// neighbour; an exact-neighbour test of its own pins that): only that SOME node
/// still holds it. Entered by the real action, walked to by Down.
#[gpui::test]
fn space_down_and_up_on_a_file_never_opens_its_diff(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("keyup-file");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Staged);
    let staged = {
        let st = status(&app, &mut vcx);
        flat_rows(&st, ScmGroup::Staged)
    };
    assert!(staged.len() > 5, "the fixture's Staged list has rows");
    let target = staged[3];
    enter(&app, &mut vcx);
    for step in 0..=12 {
        let now = marker(&mut vcx);
        assert!(
            step > 0 || now.is_some(),
            "no `ScmRows` key context is mounted at the focus"
        );
        if now.as_deref() == Some(target) {
            break;
        }
        assert!(step < 12, "Down never reached {target}, on {now:?}");
        down(&app, &mut vcx);
    }
    assert!(
        !diff_open(&app, &mut vcx, ScmGroup::Staged),
        "no diff is open before Space"
    );

    for held in [false, true] {
        space_down_up(&app, &mut vcx, held);
        assert!(
            !diff_open(&app, &mut vcx, ScmGroup::Staged),
            "Space (held: {held}) opened the file's diff on key-up"
        );
        // The keyboard is still in the list: some node holds it. NOT the row it
        // started on: a fresh Space stages that file, and staging moves the
        // keyboard to a neighbour before the row leaves the list (approved
        // behaviour, pinned by its own test). A diff overlay would take the
        // keyboard away altogether, leaving no marker.
        assert!(
            marker(&mut vcx).is_some(),
            "no `ScmRows` key context after Space (held: {held}): the keyboard left the list"
        );
    }
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}
