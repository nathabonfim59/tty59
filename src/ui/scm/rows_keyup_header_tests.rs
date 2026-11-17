//! POST-IMPLEMENTATION controls for the group header and the keyboard release,
//! written AFTER the Space-release filter in `panel.rs`. They are not RED-first
//! tests: the frozen `rows_keyup_tests` cover a directory and a file, and the
//! header shares the same filter, so these pin that the header is covered too
//! and that Enter's full pair still folds it (now once, on the press: see the
//! Enter test). A later header-only reversion of the filter is the witness that
//! these can fail.
//!
//! Events are the real pair, as in `rows_keyup_tests`: a key-down
//! (`simulate_keystrokes`, or a `KeyDownEvent` with `is_held` for a repeat) and
//! a real `gpui::KeyUpEvent` through `simulate_event`. A header folds its group
//! through `scm_toggle_group`, which records the choice in `ScmPanelState::toggled`,
//! so that set is what is read.

use gpui::{
    Entity, KeyDownEvent, KeyUpEvent, Keystroke, PlatformInput, TestAppContext, VisualTestContext,
};

use super::rows_window_tests::{big_repo, draw, header, panel_on, serial};
use crate::core::actions::ShowRightPanelChanges;
use crate::ui::app::Tty59App;
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

/// Real entry: the keyboard action puts the keyboard on the first header.
fn enter_list(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
    vcx.dispatch_action(ShowRightPanelChanges);
    draw(app, vcx);
    draw(app, vcx);
}

/// `key` as the platform delivers it: the key-down, then the key-up.
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

fn folded_by_choice(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
    app.update_in(vcx, |app, _, _| app.scm.toggled.contains(&ScmGroup::Merge))
}

/// Space, key-down and key-up, on the group header the keyboard is on: the
/// release must not fold the group.
#[cfg(unix)]
#[gpui::test]
fn space_down_and_up_on_a_group_header_does_not_fold_it(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("keyup-header");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    enter_list(&app, &mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(header(ScmGroup::Merge)),
        "the keyboard is on the first group header"
    );
    assert!(!folded_by_choice(&app, &mut vcx));
    for held in [false, true] {
        key_down_up(&app, &mut vcx, "space", held);
        assert!(
            !folded_by_choice(&app, &mut vcx),
            "Space (held: {held}) folded the group on key-up"
        );
        assert_eq!(
            marker(&mut vcx).as_deref(),
            Some(header(ScmGroup::Merge)),
            "the keyboard stays on the header"
        );
    }
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}

/// Enter, key-down and key-up, on the header the keyboard is on folds the group
/// (the full pair as the platform delivers it, as it always did). Since Enter
/// acts on the key-down (`scm_row_key`) and the nodes reject the click GPUI
/// makes from the key-up, the fold happens once, on the press: the assertion is
/// unchanged, what it exercises is the press, not the release. (Before Enter had
/// a key-down handler this was the release's click, which the Space filter had
/// left in place.)
#[cfg(unix)]
#[gpui::test]
fn enter_down_and_up_on_a_group_header_still_folds_it(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("keyup-header-enter");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    enter_list(&app, &mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(header(ScmGroup::Merge)),
        "the keyboard is on the first group header"
    );
    assert!(!folded_by_choice(&app, &mut vcx));
    key_down_up(&app, &mut vcx, "enter", false);
    assert!(
        folded_by_choice(&app, &mut vcx),
        "Enter's release no longer folds the group"
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}
