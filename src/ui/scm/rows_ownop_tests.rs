//! POST-IMPLEMENTATION controls for the keys that act (Enter, Space), written
//! AFTER `scm_row_act`. They are not RED-first tests: the frozen
//! `rows_keys_tests` pin the first behaviours (Space stage and unstage, Enter's
//! diff, discard cancel and confirm, held Space and Backspace), and this file
//! adds what they did not: that a fresh press acts exactly once while a held
//! one does nothing, that the full key-down plus key-up pair acts once, and that
//! Space moves the keyboard to the exact neighbour with every diff overlay
//! closed. Guard mutations (dropping the held guard, letting the key-up click
//! act, moving the keyboard after the dispatch) are what show they can fail.
//!
//! Events are real: `simulate_keystrokes` for a fresh key-down, a
//! `KeyDownEvent` with `is_held` for a repeat, and a real `gpui::KeyUpEvent`
//! through `simulate_event` for the release. Only the app's own state and git
//! are observed; no click is injected and no action is logged.

use gpui::{
    Entity, KeyDownEvent, KeyUpEvent, Keystroke, PlatformInput, TestAppContext, VisualTestContext,
};

use super::rows_window_tests::{
    big_repo, draw, flat_rows, git_ok, only_group, panel_on, serial, status,
};
use crate::core::actions::ShowRightPanelChanges;
use crate::ui::app::Tty59App;
use crate::ui::scm::panel::group_diff_source;
use crate::ui::scm::state::ScmGroup;

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

fn enter_list(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
    vcx.dispatch_action(ShowRightPanelChanges);
    draw(app, vcx);
    draw(app, vcx);
}

/// `key` as the platform delivers it: the key-down (a repeat when `held`), then
/// the key-up.
fn key_down_up(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, key: &str, held: bool) {
    let keystroke = Keystroke::parse(key).expect("a keystroke");
    if held {
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
        vcx.simulate_keystrokes(key);
    }
    vcx.simulate_event(KeyUpEvent { keystroke });
    draw(app, vcx);
}

fn walk_to(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, target: &str, max: usize) {
    for step in 0..=max {
        let now = marker(vcx);
        assert!(
            step > 0 || now.is_some(),
            "no `ScmRows` key context is mounted at the focus"
        );
        if now.as_deref() == Some(target) {
            return;
        }
        assert!(step < max, "Down never reached {target}, on {now:?}");
        vcx.simulate_keystrokes("down");
        draw(app, vcx);
    }
}

fn path_of(row_id: &str) -> String {
    let rest = row_id.strip_prefix("scm-row-").expect("a file row id");
    rest.split_once('-').expect("group-path").1.to_string()
}

/// Is a diff overlay open for ANY group's source?
fn any_diff_open(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
    app.update_in(vcx, |app, _, _| {
        let repo = app.scm.repo.clone().expect("a repository is on screen");
        ScmGroup::ORDER.iter().any(|group| {
            app.diff_overlay_focus(repo.host, &repo.root, &group_diff_source(*group))
                .is_some()
        })
    })
}

fn pump_until(vcx: &mut VisualTestContext, secs: u64, mut until: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        vcx.run_until_parked();
        if until() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Enter on a group header the keyboard is on: a fresh press folds it once, a
/// held (repeated) press does nothing, and the release never folds it a second
/// time, so each fresh press toggles exactly once.
#[gpui::test]
fn a_fresh_enter_folds_a_header_once_and_a_held_one_does_nothing(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("ownop-header-enter");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    enter_list(&app, &mut vcx);
    let collapsed = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        app.update_in(vcx, |app, _, _| {
            app.scm.collapsed.contains(&ScmGroup::Merge)
        })
    };
    assert!(
        marker(&mut vcx).is_some(),
        "no `ScmRows` key context is mounted at the focus"
    );
    assert!(!collapsed(&app, &mut vcx));
    key_down_up(&app, &mut vcx, "enter", false);
    assert!(collapsed(&app, &mut vcx), "a fresh Enter folded the group");
    key_down_up(&app, &mut vcx, "enter", true);
    assert!(
        collapsed(&app, &mut vcx),
        "a held Enter changed the group's fold"
    );
    key_down_up(&app, &mut vcx, "enter", false);
    assert!(
        !collapsed(&app, &mut vcx),
        "a second fresh Enter did not open it again: the pair acted more than once"
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}

/// Enter on a tree directory: the same, once per fresh press, nothing held.
#[gpui::test]
fn a_fresh_enter_folds_a_directory_once_and_a_held_one_does_nothing(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("ownop-dir-enter");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
    only_group(&app, &mut vcx, ScmGroup::Staged);
    enter_list(&app, &mut vcx);
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
        vcx.simulate_keystrokes("down");
        draw(&app, &mut vcx);
    }
    let dir = dir.expect("Down reached a Staged directory row");
    let folds = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        app.update_in(vcx, |app, _, _| {
            app.scm
                .folded_dirs
                .iter()
                .filter(|(g, _)| *g == ScmGroup::Staged)
                .count()
        })
    };
    assert_eq!(folds(&app, &mut vcx), 0);
    key_down_up(&app, &mut vcx, "enter", false);
    assert_eq!(
        folds(&app, &mut vcx),
        1,
        "a fresh Enter folded the directory"
    );
    key_down_up(&app, &mut vcx, "enter", true);
    assert_eq!(folds(&app, &mut vcx), 1, "a held Enter changed the fold");
    key_down_up(&app, &mut vcx, "enter", false);
    assert_eq!(
        folds(&app, &mut vcx),
        0,
        "a second fresh Enter did not open it again: the pair acted more than once"
    );
    assert_eq!(marker(&mut vcx).as_deref(), Some(dir.as_str()));
    app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}

/// Space (full key-down plus key-up) on an unstaged file: its change is
/// staged and nothing else is; the keyboard is on the EXACT next row of the
/// list as it was before the press; no diff overlay of any group is open. A
/// held Space afterwards stages nothing.
#[gpui::test]
fn space_stages_the_row_and_moves_the_keyboard_to_the_exact_neighbour(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("ownop-space-stage");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Changes);
    let changes = {
        let st = status(&app, &mut vcx);
        flat_rows(&st, ScmGroup::Changes)
    };
    enter_list(&app, &mut vcx);
    walk_to(&app, &mut vcx, changes[5], 12);
    let (here, next, after) = (
        path_of(changes[5]),
        path_of(changes[6]),
        path_of(changes[7]),
    );
    // Nothing is staged for these paths yet: the index equals HEAD for them.
    let unstaged = |path: &str| git_ok(&root, &["diff", "--cached", "--quiet", "--", path]);
    assert!(unstaged(&here) && unstaged(&next) && unstaged(&after));

    key_down_up(&app, &mut vcx, "space", false);
    assert!(
        pump_until(&mut vcx, 30, || !unstaged(&here)),
        "Space never staged {here}"
    );
    assert!(unstaged(&next), "the neighbour {next} was staged too");
    assert!(!any_diff_open(&app, &mut vcx), "a diff overlay is open");
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(changes[6]),
        "the keyboard is not on the exact next row"
    );

    key_down_up(&app, &mut vcx, "space", true);
    let _ = pump_until(&mut vcx, 2, || false);
    assert!(unstaged(&next), "a held Space staged {next}");
    assert!(unstaged(&after), "a held Space staged {after}");
    assert_eq!(marker(&mut vcx).as_deref(), Some(changes[6]));
    assert!(!any_diff_open(&app, &mut vcx), "a diff overlay is open");
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}

/// The same for Space on a staged file: it is unstaged, the keyboard is on the
/// exact next row, and no diff overlay is open.
#[gpui::test]
fn space_unstages_the_row_and_moves_the_keyboard_to_the_exact_neighbour(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("ownop-space-unstage");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Staged);
    let staged = {
        let st = status(&app, &mut vcx);
        flat_rows(&st, ScmGroup::Staged)
    };
    enter_list(&app, &mut vcx);
    walk_to(&app, &mut vcx, staged[5], 12);
    let (here, next) = (path_of(staged[5]), path_of(staged[6]));

    key_down_up(&app, &mut vcx, "space", false);
    assert!(
        pump_until(&mut vcx, 30, || git_ok(
            &root,
            &["diff", "--cached", "--quiet", "--", &here]
        )),
        "Space never unstaged {here}"
    );
    assert!(
        !git_ok(&root, &["diff", "--cached", "--quiet", "--", &next]),
        "the neighbour {next} was unstaged too"
    );
    assert!(!any_diff_open(&app, &mut vcx), "a diff overlay is open");
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(staged[6]),
        "the keyboard is not on the exact next row"
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}
