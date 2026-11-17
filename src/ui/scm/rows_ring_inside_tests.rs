//! Where the keyboard ring of a roving list row lies, written BEFORE the
//! placement change.
//!
//! THE BUG. The Changes list and the Info table each keep one keyboard stop.
//! That stop wears the shared `FocusRing` with `RingPlacement::Outside`: the
//! ring is drawn out past the row's edge by the stroke and the theme's gap. A
//! scroller clips its content, and the minimal reveal that Down, Up, Home and
//! End use leaves the row flush with the scroller's top or bottom edge, so the
//! outer part of the ring on that side is cut off. The fix is to place these two
//! rings `Inside` (the placement the Info body ring already uses), which keeps
//! the ring within the row's own box and so within whatever shows the row.
//!
//! WHAT THE TESTS READ. Nothing a production planner computed:
//!
//! * the live viewport is gpui's own `ScrollHandle::bounds()` of the scroller
//!   that really holds the rows (`scm.scroll`, `right_panel.scroll`);
//! * the ring and its own frame are the debug bounds of the two elements the
//!   existing `FocusRing` names under a test build (`.tagged`): the ring is
//!   `<tag>`, the wrapper around the row is `<tag>-frame`;
//! * the row itself is the selector the row already carries.
//!
//! Every assertion is made in an ACTIVE window (asserted before the keyboard
//! enters), after real keys (`simulate_keystrokes`, so the last input is the
//! keyboard, which the ring requires) and a frame, in this order, so a red test
//! names the first thing that is wrong: the keyboard is on the expected node
//! (the key-context marker; for the Changes rows also the row handle's own
//! focus), its own row is fully inside the live viewport, its frame wraps that
//! row, the ring is drawn, the ring is inside the live viewport on all four
//! sides, and only then the ring is inside its own frame on all four sides.
//!
//! The Info rows keep their roving handle private, so for them the marker, the
//! keyboard modality and the mounted ring establish the box under test; no
//! handle assertion is made there, and the existing Info keyboard tests cover
//! its focus semantics.
//!
//! "Inside the viewport" is the user's symptom, red wherever the reveal leaves
//! the row against a scroller edge, and it names that edge. "Inside its own
//! frame" is the placement policy and is red today wherever the keyboard is,
//! because an outer ring is by definition outside the box it marks; it is
//! reached only when the symptom check passed.
//!
//! The fixture cleanup uses `std::fs`; this exact file and that literal pattern
//! are in the allowlist of `.github/scripts/check-host-boundary.sh` with the
//! reason (a scratch repository under the machine's temp directory).
//!
//! WHAT THEY DO NOT PROVE: the look of the ring on a selected row (a native
//! capture decides that), nor anything about other rings.

use gpui::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext};

use super::rows_window_tests::{
    big_repo, draw, flat_rows, header, only_group, panel_on, pitch, row_budget, scroll_to,
    scroll_top, serial, status, viewport,
};
use crate::core::actions::{ShowRightPanelChanges, ShowRightPanelInfo};
use crate::ui::app::Tty59App;
use crate::ui::scm::state::ScmGroup;

/// Sub-pixel slack: layout rounds to device pixels.
const TOL: f32 = 0.5;

const SCM_RING: &str = "scm-stop-ring";
const SCM_FRAME: &str = "scm-stop-ring-frame";
const INFO_RING: &str = "info-stop-ring";
const INFO_FRAME: &str = "info-stop-ring-frame";

// ---------------------------------------------------------------- helpers --

/// The `row` value of the `ScmRows` key context on the focus path: the Changes
/// node that holds the keyboard.
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

/// The `key` value of the `InfoRow` key context on the focus path.
fn info_marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|window, _| {
        window
            .context_stack()
            .iter()
            .find(|c| c.contains("InfoRow"))
            .and_then(|c| c.get("key"))
            .map(|v| v.to_string())
    })
}

/// A real key, then two frames: the second one settles a reveal that the first
/// frame measured.
fn press(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    draw(app, vcx);
    draw(app, vcx);
}

/// Activate the window (asserted), then put the keyboard on the active
/// terminal, the way the app does on its own.
fn focus_terminal(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    activate(vcx);
    app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
}

/// The real keyboard entry into the Changes tab.
fn enter(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    vcx.dispatch_action(ShowRightPanelChanges);
    draw(app, vcx);
    draw(app, vcx);
}

/// Press `key` until the keyboard is on the node `target`, at most `max` times.
fn walk(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, key: &str, target: &str, max: usize) {
    for step in 0..=max {
        if marker(vcx).as_deref() == Some(target) {
            return;
        }
        if step == max {
            break;
        }
        press(app, vcx, key);
    }
    panic!(
        "pressing {key} {max} times never reached {target}; the keyboard is on {:?}",
        marker(vcx)
    );
}

fn clean_up(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(root));
}

/// Every side of `inner` lies within `outer`, one assertion per side.
fn assert_within(
    what: &str,
    inner_name: &str,
    inner: Bounds<Pixels>,
    outer_name: &str,
    outer: Bounds<Pixels>,
) {
    let slack = [
        ("top", inner.top().as_f32() - outer.top().as_f32()),
        ("left", inner.left().as_f32() - outer.left().as_f32()),
        ("right", outer.right().as_f32() - inner.right().as_f32()),
        ("bottom", outer.bottom().as_f32() - inner.bottom().as_f32()),
    ];
    for (side, slack) in slack {
        assert!(
            slack >= -TOL,
            "{what}: the {inner_name}'s {side} edge is {:.2}px outside the {outer_name} \
             ({inner_name} {inner:?}, {outer_name} {outer:?})",
            -slack
        );
    }
}

/// The ring checks, in the order that makes a red test name its own cause.
/// `own` is the row's own bounds where the row carries a selector, and `view`
/// the live viewport of the scroller that holds it.
fn check_ring(
    vcx: &mut VisualTestContext,
    what: &str,
    own: Option<Bounds<Pixels>>,
    view: Bounds<Pixels>,
    (ring_sel, frame_sel): (&'static str, &'static str),
) {
    let frame = vcx
        .debug_bounds(frame_sel)
        .unwrap_or_else(|| panic!("{what}: the focused row's ring frame is not drawn"));
    match own {
        // The row first: if it is not fully in view, the ring cannot be blamed.
        Some(own) => {
            assert_within(what, "row", own, "live viewport", view);
            assert_within(what, "row", own, "ring frame", frame);
        }
        None => assert_within(what, "ring frame", frame, "live viewport", view),
    }
    let ring = vcx.debug_bounds(ring_sel).unwrap_or_else(|| {
        panic!("{what}: no keyboard ring is drawn on the row after keyboard input")
    });
    // The symptom first: what the user sees cut off by the scroller. A red
    // here names the clipped viewport edge, not a placement policy.
    assert_within(what, "ring", ring, "live viewport", view);
    // Then the policy: the ring is within the box it marks.
    assert_within(what, "ring", ring, "own frame", frame);
}

/// A test window is INACTIVE until the platform says otherwise, and gpui builds
/// its focus events from empty paths while the window is inactive, so the
/// focus the keys below move would not be the one a person has. Made active the
/// supported way and asserted before the keyboard enters the list.
fn activate(vcx: &mut VisualTestContext) {
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    assert!(
        vcx.update(|window, _| window.is_window_active()),
        "setup: the test window is active before the keyboard enters"
    );
}

fn keyboard_was_last(vcx: &mut VisualTestContext, what: &str) {
    assert!(
        vcx.update(|window, _| window.last_input_was_keyboard()),
        "{what}: the last input was not the keyboard"
    );
}

/// The Changes node `id` holds the keyboard, from real keys, and its ring lies
/// inside its frame and the live viewport.
fn check_scm(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, what: &str, id: &str) {
    assert_eq!(
        marker(vcx).as_deref(),
        Some(id),
        "{what}: the keyboard is not on the expected node"
    );
    assert!(
        app.update_in(vcx, |app, window, _| app
            .scm_row_focus
            .handle
            .is_focused(window)),
        "{what}: the row handle does not hold the focus"
    );
    keyboard_was_last(vcx, what);
    let sel: &'static str = Box::leak(id.to_string().into_boxed_str());
    let own = vcx
        .debug_bounds(sel)
        .unwrap_or_else(|| panic!("{what}: the focused node {id} is not drawn"));
    let view = viewport(app, vcx);
    check_ring(vcx, what, Some(own), view, (SCM_RING, SCM_FRAME));
}

/// `(offset from the top, the most it can scroll)` of the Changes scroller.
fn scm_scroll(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> (f32, f32) {
    let top = scroll_top(app, vcx);
    let max = app.update_in(vcx, |app, _, _| {
        app.scm.scroll.max_offset().y.as_f32().abs()
    });
    (top, max)
}

/// Scrolled strictly between the two ends, so neither end explains the result.
fn assert_middle(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, what: &str) {
    let (top, max) = scm_scroll(app, vcx);
    assert!(
        top > TOL && top < max - TOL,
        "{what}: the list is not scrolled between its ends (offset {top}, most {max})"
    );
}

// ------------------------------------------------------------ Changes list --

/// At the top of the list: the first header, the next header and the first file
/// row, each reached by the real keys from the entry, at scroll offset 0.
#[cfg(unix)]
#[gpui::test]
fn the_ring_stays_inside_at_the_top_of_the_changes_list(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("ring-top");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Staged);
    let staged = flat_rows(&status(&app, &mut vcx), ScmGroup::Staged);
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    // The entry is a dispatched action, not a key: Home (a real key, to the
    // first node the keyboard is already on) gives the setup genuine keyboard
    // modality before anything is checked.
    press(&app, &mut vcx, "home");

    check_scm(&app, &mut vcx, "Merge header", header(ScmGroup::Merge));
    assert!(
        scm_scroll(&app, &mut vcx).0 <= TOL,
        "the list is at its top at the entry"
    );
    press(&app, &mut vcx, "down");
    check_scm(&app, &mut vcx, "Staged header", header(ScmGroup::Staged));
    assert!(scm_scroll(&app, &mut vcx).0 <= TOL, "still at the top");
    press(&app, &mut vcx, "down");
    check_scm(&app, &mut vcx, "first Staged row", staged[0]);
    clean_up(&root);
}

/// In the middle of the list, reached by Down (the reveal leaves the row flush
/// with the bottom edge) and then by Up (flush with the top edge), and one row
/// that is off both edges.
#[cfg(unix)]
#[gpui::test]
fn the_ring_stays_inside_in_the_middle_of_the_changes_list(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("ring-middle");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Staged);
    let staged = flat_rows(&status(&app, &mut vcx), ScmGroup::Staged);
    let k = row_budget(&app, &mut vcx) + 15;
    let fits = (viewport(&app, &mut vcx).size.height.as_f32() / pitch(&mut vcx)).ceil() as usize;
    assert!(
        k > fits + 8 && k + 8 < staged.len(),
        "the list is long enough"
    );
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);

    walk(&app, &mut vcx, "down", staged[k], k + 8);
    assert_middle(&app, &mut vcx, "after walking Down");
    check_scm(&app, &mut vcx, "row reached by Down", staged[k]);

    for _ in 0..3 {
        press(&app, &mut vcx, "up");
    }
    assert_middle(&app, &mut vcx, "after three Up");
    check_scm(
        &app,
        &mut vcx,
        "row three above, off the edges",
        staged[k - 3],
    );

    let j = k - 3 - fits - 2;
    walk(&app, &mut vcx, "up", staged[j], fits + 4);
    assert_middle(&app, &mut vcx, "after walking Up past a viewport");
    check_scm(&app, &mut vcx, "row reached by Up", staged[j]);
    clean_up(&root);
}

/// At the bottom of the list, End and then Home: the last row of the last group
/// at the end of the scroll, and the first header back at offset 0.
#[cfg(unix)]
#[gpui::test]
fn the_ring_stays_inside_at_the_bottom_of_the_changes_list_and_home_returns(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = big_repo("ring-bottom");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Changes);
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);

    press(&app, &mut vcx, "end");
    let last = marker(&mut vcx).expect("End put the keyboard on a node");
    assert!(
        last.starts_with("scm-row-Changes-"),
        "End did not reach a row of the last expanded group: {last}"
    );
    let (top, max) = scm_scroll(&app, &mut vcx);
    assert!(top > TOL && top <= max + TOL, "End scrolled the list");
    check_scm(&app, &mut vcx, "last row after End", &last);

    // And with the list scrolled all the way down, the same row still drawn.
    scroll_to(&app, &mut vcx, 1e7);
    let (top, max) = scm_scroll(&app, &mut vcx);
    assert!(
        (top - max).abs() <= TOL && max > TOL,
        "the list is at the end of its scroll (offset {top}, most {max})"
    );
    check_scm(&app, &mut vcx, "last row at the end of the scroll", &last);

    press(&app, &mut vcx, "home");
    check_scm(
        &app,
        &mut vcx,
        "first header after Home",
        header(ScmGroup::Merge),
    );
    assert!(
        scm_scroll(&app, &mut vcx).0 <= TOL,
        "Home scrolled the list back to its top"
    );
    clean_up(&root);
}

// --------------------------------------------------------------- Info rows --

/// The big fixture's pane with the Info tab asked for by the real action and
/// its branch row drawn, the keyboard on the Info root.
fn info_on_repo(
    cx: &mut TestAppContext,
    name: &str,
) -> (
    Entity<Tty59App>,
    VisualTestContext,
    crate::daemon::transport::Stream,
    std::path::PathBuf,
) {
    let root = big_repo(name);
    let (app, mut vcx, pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    vcx.dispatch_action(ShowRightPanelInfo);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        app.update_in(&mut vcx, |_, _, cx| cx.notify());
        draw(&app, &mut vcx);
        if vcx.debug_bounds("panel-info-branch-row").is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the Info table never drew its branch row"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    draw(&app, &mut vcx);
    assert_eq!(info_marker(&mut vcx), None, "the root holds the keyboard");
    (app, vcx, pane, root)
}

/// The Info row `key` holds the keyboard, from real keys, and its ring lies
/// inside its frame and the live viewport of the Info scroller. The row has no
/// selector of its own, so its frame is what is proved to be in view first.
fn check_info(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, what: &str, key: &str) {
    assert_eq!(
        info_marker(vcx).as_deref(),
        Some(key),
        "{what}: the keyboard is not on the expected Info row"
    );
    keyboard_was_last(vcx, what);
    let view = app.update_in(vcx, |app, _, _| app.right_panel.scroll.bounds());
    check_ring(vcx, what, None, view, (INFO_RING, INFO_FRAME));
}

/// Pane and working directory by Down, the last stop by End and the first by
/// Home: the same two properties for the Info rows, which wear the same ring.
#[cfg(unix)]
#[gpui::test]
fn the_ring_stays_inside_on_the_info_rows(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, _pane, root) = info_on_repo(cx, "ring-info");

    press(&app, &mut vcx, "down");
    check_info(&app, &mut vcx, "Pane row after Down", "Pane");
    press(&app, &mut vcx, "down");
    check_info(&app, &mut vcx, "Cwd row after Down", "Cwd");
    press(&app, &mut vcx, "end");
    check_info(&app, &mut vcx, "last row after End", "Changes");
    press(&app, &mut vcx, "home");
    check_info(&app, &mut vcx, "first row after Home", "Pane");
    clean_up(&root);
}
