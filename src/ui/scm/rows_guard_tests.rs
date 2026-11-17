//! POST-IMPLEMENTATION guard and scope observations for the keys that act,
//! written AFTER the S4 hook (`scm_op_approved`'s `OnApproved`) and the key
//! handler (`scm_row_act`). They are not RED-first: they are expected GREEN on
//! the code as it is, and are made meaningful by later guard mutations: bypass
//! the hook's row/generation conjunction (the row-moved test moves both the
//! row and the generation, so it kills that conjunction as a whole, not either
//! check alone), drop the exact-focus check (the focus-only test isolates it),
//! and use the unfiltered entries (the two filter tests).
//!
//! TEST-PLATFORM PROMPT, honestly labelled: the platform prompt of the test
//! platform does not block input, so "the user moves while the prompt is up" is
//! delivered as real keys and a real focus change between the Backspace and the
//! answer. That is not a claim that a native prompt lets a user do this in
//! production; only that if they do, the hook cannot override them.
//!
//! No fake log: the real events are `simulate_keystrokes` and `simulate_input`,
//! the real prompt answer is `simulate_prompt_answer`, and the only things read
//! are the keyboard (the `ScmRows` key context), `ScmPanelState` and real git
//! (`git diff --name-only`, `git diff --cached --name-only`). No prompt wording
//! is asserted. The scope of B1 and B2 is NOT taken from `scm_group_entries` or
//! `flat_rows`: the expected path set comes from real git, with a literal
//! `pkg_13/` prefix, before the operation.

use std::collections::BTreeSet;
use std::path::Path;

use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};

use super::rows_window_tests::{
    big_repo, draw, flat_rows, git_stdout, only_group, panel_on, serial, status,
};
use crate::core::actions::ShowRightPanelChanges;
use crate::ui::app::Tty59App;
use crate::ui::i18n::{L10nKey, t};
use crate::ui::scm::panel::group_diff_source;
use crate::ui::scm::state::{NodeKind, ScmGroup};

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

/// The set of lines real git prints for `args` in `root`.
fn git_set(root: &Path, args: &[&str]) -> BTreeSet<String> {
    git_stdout(root, args).lines().map(str::to_string).collect()
}

/// Paths with unstaged changes (what a discard of the Changes group reverts).
fn unstaged(root: &Path) -> BTreeSet<String> {
    git_set(root, &["diff", "--name-only"])
}

/// Paths whose index differs from HEAD (what staging adds to).
fn staged(root: &Path) -> BTreeSet<String> {
    git_set(root, &["diff", "--cached", "--name-only"])
}

fn any_diff_open(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
    app.update_in(vcx, |app, _, _| {
        let repo = app.scm.repo.clone().expect("a repository is on screen");
        ScmGroup::ORDER.iter().any(|group| {
            app.diff_overlay_focus(repo.host, &repo.root, &group_diff_source(*group))
                .is_some()
        })
    })
}

fn pump_until(
    vcx: &mut VisualTestContext,
    secs: u64,
    mut until: impl FnMut(&mut VisualTestContext) -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        vcx.run_until_parked();
        if until(vcx) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn filter_handle(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> gpui::FocusHandle {
    app.update_in(vcx, |app, _, cx| {
        app.scm.filter.as_ref().map(|f| f.read(cx).focus_handle(cx))
    })
    .expect("the filter field exists")
}

/// Walk to `changes[5]`, press Backspace so the prompt is pending, and return
/// the rows and the paths at 5, 6 and 7 with what real git has unstaged now.
fn prompt_open_on_changes_row(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
) -> (Vec<&'static str>, [String; 3], BTreeSet<String>) {
    only_group(app, vcx, ScmGroup::Changes);
    let changes = {
        let st = status(app, vcx);
        flat_rows(&st, ScmGroup::Changes)
    };
    enter_list(app, vcx);
    walk_to(app, vcx, changes[5], 12);
    let paths = [
        path_of(changes[5]),
        path_of(changes[6]),
        path_of(changes[7]),
    ];
    vcx.simulate_keystrokes("backspace");
    assert!(
        pump_until(vcx, 5, |vcx| vcx.has_pending_prompt()),
        "Backspace never asked for approval"
    );
    let before = unstaged_of(app, vcx);
    (changes, paths, before)
}

fn unstaged_of(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> BTreeSet<String> {
    let root = app.update_in(vcx, |app, _, _| {
        app.scm
            .repo
            .clone()
            .expect("a repository is on screen")
            .root
    });
    unstaged(&root)
}

/// A1: with the prompt pending the user moves the keyboard TWO rows down, then
/// confirms. Exactly the approved path is discarded (real git), the keyboard is
/// where the user left it (changes[7]) and NOT on the neighbour the hook would
/// have chosen (changes[6]).
#[gpui::test]
fn a_row_moved_during_the_prompt_is_not_moved_back_and_only_the_approved_path_goes(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = big_repo("guard-row-moved");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    crate::ui::i18n::set_locale("en");
    let (changes, [here, next, after], before) = prompt_open_on_changes_row(&app, &mut vcx);
    assert!(before.contains(&here) && before.contains(&next) && before.contains(&after));

    vcx.simulate_keystrokes("down");
    draw(&app, &mut vcx);
    vcx.simulate_keystrokes("down");
    draw(&app, &mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(changes[7]),
        "the user moved two rows down while the prompt was up"
    );

    vcx.simulate_prompt_answer(t(L10nKey::ScmDiscard));
    assert!(
        pump_until(&mut vcx, 30, |_| !unstaged(&root).contains(&here)),
        "the approved path was never discarded"
    );
    let left: BTreeSet<String> = before.difference(&unstaged(&root)).cloned().collect();
    assert_eq!(
        left,
        BTreeSet::from([here.clone()]),
        "exactly the approved path was discarded"
    );
    assert!(!vcx.has_pending_prompt(), "one prompt, answered");
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(changes[7]),
        "the keyboard was moved back to the neighbour {next} after the user had left it"
    );
    assert!(!any_diff_open(&app, &mut vcx));
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}

/// A2: with the prompt pending the user moves the FOCUS to the filter field and
/// leaves the row alone, then confirms. This isolates the exact-focus condition
/// of the hook from the generation and row conditions: the approved path is
/// still discarded, the keyboard stays in the filter field and typing reaches
/// it, and the list's keyboard node is NOT moved to the neighbour.
#[gpui::test]
fn focus_moved_to_the_filter_during_the_prompt_is_not_taken_back(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("guard-focus-moved");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    crate::ui::i18n::set_locale("en");
    let (_changes, [here, next, _after], before) = prompt_open_on_changes_row(&app, &mut vcx);
    let row_before = app.update_in(&mut vcx, |app, _, _| app.scm.row_at.clone());
    assert!(
        matches!(row_before.as_ref().map(|r| &r.node), Some(NodeKind::File(p)) if *p == here),
        "the keyboard node is the row the key was pressed on"
    );

    let filter = filter_handle(&app, &mut vcx);
    vcx.update(|window, cx| window.focus(&filter, cx));
    draw(&app, &mut vcx);
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(filter.clone())
    );

    vcx.simulate_prompt_answer(t(L10nKey::ScmDiscard));
    assert!(
        pump_until(&mut vcx, 30, |_| !unstaged(&root).contains(&here)),
        "the approved path was never discarded"
    );
    let left: BTreeSet<String> = before.difference(&unstaged(&root)).cloned().collect();
    assert_eq!(
        left,
        BTreeSet::from([here.clone()]),
        "exactly the approved path was discarded"
    );
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(filter.clone()),
        "the keyboard was taken from the filter field"
    );
    // The landing may drop the discarded row from the keyboard (an orphan is
    // never kept), so what is asserted is that the neighbour was NOT taken.
    let row_after = app.update_in(&mut vcx, |app, _, _| app.scm.row_at.clone());
    assert!(
        !matches!(row_after.as_ref().map(|r| &r.node), Some(NodeKind::File(p)) if *p == next),
        "the keyboard node was moved to the neighbour {next} although the user had left the list: {row_after:?}"
    );
    assert_eq!(marker(&mut vcx), None, "no list node holds the keyboard");
    vcx.simulate_input("q");
    let typed = app.update_in(&mut vcx, |app, _, cx| {
        app.scm
            .filter
            .as_ref()
            .map(|f| f.read(cx).value().to_string())
    });
    assert_eq!(
        typed.as_deref(),
        Some("q"),
        "typing reaches the filter field"
    );
    assert!(!any_diff_open(&app, &mut vcx));
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}

/// The paths real git has changed (unstaged) under the literal `pkg_13/`
/// prefix, and one under `pkg_14/` as the hidden control, asserted non-empty.
fn shown_and_hidden(root: &Path) -> (BTreeSet<String>, String) {
    let all = unstaged(root);
    let shown: BTreeSet<String> = all
        .iter()
        .filter(|p| p.starts_with("pkg_13/"))
        .cloned()
        .collect();
    assert!(!shown.is_empty(), "the fixture changes files under pkg_13/");
    let hidden = all
        .iter()
        .find(|p| p.starts_with("pkg_14/"))
        .cloned()
        .expect("the fixture changes a file under pkg_14/, the hidden control");
    (shown, hidden)
}

/// Type `pkg_13` into the real filter field and enter the list by the real
/// action: the filter hides every group but Changes, so the first header is its.
fn filter_to_pkg_13_and_enter(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    let filter = filter_handle(app, vcx);
    vcx.update(|window, cx| window.focus(&filter, cx));
    vcx.simulate_input("pkg_13");
    draw(app, vcx);
    enter_list(app, vcx);
    assert_eq!(
        marker(vcx).as_deref(),
        Some("scm-group-Changes"),
        "with the filter on, the first header is Changes"
    );
}

/// B1: header Space with the filter on stages exactly the shown paths: the set
/// real git gained in the index equals the shown set, the hidden control gained
/// nothing, and nothing the filter hid was touched.
#[gpui::test]
fn header_space_with_a_filter_stages_exactly_the_shown_paths(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("guard-filter-stage");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    let (shown, hidden) = shown_and_hidden(&root);
    let before = staged(&root);
    assert!(
        shown.iter().all(|p| !before.contains(p)),
        "none of the shown paths is staged yet"
    );
    filter_to_pkg_13_and_enter(&app, &mut vcx);

    vcx.simulate_keystrokes("space");
    assert!(
        pump_until(&mut vcx, 30, |_| shown.is_subset(&staged(&root))),
        "Space never staged the shown paths"
    );
    let _ = pump_until(&mut vcx, 2, |_| false);
    let after = staged(&root);
    let gained: BTreeSet<String> = after.difference(&before).cloned().collect();
    assert_eq!(gained, shown, "exactly the shown paths were staged");
    assert!(
        before.is_subset(&after),
        "nothing that was staged was unstaged"
    );
    assert!(
        !after.contains(&hidden),
        "the hidden path {hidden} was staged"
    );
    assert!(!any_diff_open(&app, &mut vcx));
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}

/// B2: header Backspace with the filter on, confirmed: exactly the shown paths
/// are discarded, the hidden control and every other changed path stay changed.
/// One prompt, answered once.
#[gpui::test]
fn header_backspace_with_a_filter_discards_exactly_the_shown_paths(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("guard-filter-discard");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    crate::ui::i18n::set_locale("en");
    let (shown, hidden) = shown_and_hidden(&root);
    let before = unstaged(&root);
    filter_to_pkg_13_and_enter(&app, &mut vcx);

    vcx.simulate_keystrokes("backspace");
    assert!(
        pump_until(&mut vcx, 5, |vcx| vcx.has_pending_prompt()),
        "Backspace on the header never asked for approval"
    );
    assert_eq!(
        unstaged(&root),
        before,
        "nothing is discarded before the answer"
    );
    vcx.simulate_prompt_answer(t(L10nKey::ScmDiscard));
    assert!(
        pump_until(&mut vcx, 30, |_| shown.is_disjoint(&unstaged(&root))),
        "the shown paths were never discarded"
    );
    let _ = pump_until(&mut vcx, 2, |_| false);
    let after = unstaged(&root);
    let reverted: BTreeSet<String> = before.difference(&after).cloned().collect();
    assert_eq!(reverted, shown, "exactly the shown paths were discarded");
    assert!(
        after.contains(&hidden),
        "the hidden path {hidden} was discarded"
    );
    assert!(after.is_subset(&before), "something new became changed");
    assert!(!vcx.has_pending_prompt(), "one prompt, answered once");
    assert!(!any_diff_open(&app, &mut vcx));
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}
