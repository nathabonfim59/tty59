//! Keyboard reveal of a Keyboard Shortcuts cell: a real Tab to a cell that starts
//! below the fold of the settings page brings it (and its keyboard ring) into the
//! viewport. TESTS ONLY, written before the cell opts into the shared reveal, so
//! the reach tests are expected RED until it does.
//!
//! These use only what the parent's shortcut cells already carry: the selectors
//! `kb-{action}` (cell) and `kb-ring-{action}` (its keyboard ring, mounted only
//! while the keyboard holds focus). They need the parent's cell (its own focus
//! handle, tab stop and tagged ring), so they run on a tree that has it.
//!
//! * the window is ACTIVATED: the reveal is a focus-in listener and an inactive
//!   test window delivers no focus events;
//! * the target is the nearest cell that starts entirely below the viewport, proved
//!   from its own debug bounds at offset 0 BEFORE any focus moves;
//! * Tab is a real key, followed by `run_until_parked` ONLY, no explicit draw;
//! * which cell holds the keyboard is read from its ring, so no marker is needed.

use gpui::{
    Bounds, KeyUpEvent, Keystroke, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size,
};

use super::kit::test_support::harness as raw_harness;
use super::*;

const MAX_TABS: usize = 400;
const NARROW: (f32, f32) = (960., 420.);

struct Cell {
    action: String,
    cell: &'static str,
    ring: &'static str,
}

fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..3 {
        vcx.run_until_parked();
        vcx.update(|w, cx| {
            let _ = w.draw(cx);
        });
    }
}

/// A key down and up, then explicit draws: used only where the scroll is not the
/// claim.
fn press(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    vcx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).expect("a key name"),
    });
    settle(vcx);
}

fn viewport(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Bounds<Pixels> {
    app.update_in(vcx, |app, _, _| {
        app.active_settings()
            .expect("settings open")
            .content_scroll
            .bounds()
    })
}

fn offset_y(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Pixels {
    app.update_in(vcx, |app, _, _| {
        app.active_settings()
            .expect("settings open")
            .content_scroll
            .offset()
            .y
    })
}

fn within(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let slack = px(0.5);
    inner.top() >= outer.top() - slack
        && inner.bottom() <= outer.bottom() + slack
        && inner.left() >= outer.left() - slack
        && inner.right() <= outer.right() + slack
}

fn recording(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Option<String> {
    app.update_in(vcx, |app, _, _| {
        app.active_settings()
            .and_then(|s| s.recording.as_ref())
            .map(|r| r.action.clone())
    })
}

/// Every action the page lists, with its cell and ring selectors (leaked:
/// `debug_bounds` takes `&'static str`; this is test code).
fn cells(vcx: &mut VisualTestContext) -> Vec<Cell> {
    let actions: Vec<String> = vcx.update(|_, cx| {
        crate::ui::keymap::effective_chords(cx)
            .into_iter()
            .map(|(action, _)| action)
            .collect()
    });
    actions
        .into_iter()
        .map(|action| {
            let cell: &'static str = Box::leak(format!("kb-{action}").into_boxed_str());
            let ring: &'static str = Box::leak(format!("kb-ring-{action}").into_boxed_str());
            Cell { action, cell, ring }
        })
        .collect()
}

/// The action whose keyboard ring is mounted right now.
fn focused_cell(vcx: &mut VisualTestContext, cells: &[Cell]) -> Option<String> {
    cells
        .iter()
        .find(|c| vcx.debug_bounds(c.ring).is_some())
        .map(|c| c.action.clone())
}

/// Real Tabs, each followed by `run_until_parked` only, until `action`'s ring is
/// mounted.
fn tab_to(vcx: &mut VisualTestContext, cells: &[Cell], action: &str) -> bool {
    for _ in 0..MAX_TABS {
        if focused_cell(vcx, cells).as_deref() == Some(action) {
            return true;
        }
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
    }
    focused_cell(vcx, cells).as_deref() == Some(action)
}

/// The Keyboard Shortcuts page in an ACTIVATED narrow window with its search field
/// focused, plus the cell that starts nearest below the fold, proved from its own
/// bounds before any focus moves.
struct Fixture {
    app: Entity<Tty59App>,
    vcx: VisualTestContext,
    cells: Vec<Cell>,
    target: usize,
}

fn fixture(cx: &mut TestAppContext) -> Fixture {
    use gpui::Focusable as _;
    let (app, mut vcx) = raw_harness(cx);
    vcx.update(|w, _| w.activate_window());
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| w.is_window_active()),
        "setup: the test window is active"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Keybindings, window, cx);
    });
    vcx.simulate_resize(size(px(NARROW.0), px(NARROW.1)));
    settle(&mut vcx);

    let cells = cells(&mut vcx);
    let view = viewport(&app, &mut vcx);
    assert!(view.size.height > px(0.), "setup: the page is laid out");
    assert_eq!(
        offset_y(&app, &mut vcx),
        px(0.),
        "setup: the page is at its top"
    );
    let target = cells
        .iter()
        .enumerate()
        .filter_map(|(i, c)| vcx.debug_bounds(c.cell).map(|b| (i, b)))
        .filter(|(_, b)| b.top() >= view.bottom())
        .min_by(|a, b| {
            f32::from(a.1.top())
                .partial_cmp(&f32::from(b.1.top()))
                .expect("finite")
        })
        .map(|(i, _)| i)
        .expect("setup: some cell starts entirely below the viewport");
    let b = vcx.debug_bounds(cells[target].cell).expect("drawn");
    assert!(
        !within(b, view) && b.top() >= view.bottom(),
        "setup, before any focus moves: {} starts below the fold ({b:?} vs {view:?})",
        cells[target].action
    );

    let handle = app.update_in(&mut vcx, |app, window, cx| {
        let search = app
            .active_settings()
            .expect("settings open")
            .shortcut_search
            .clone();
        search.update(cx, |s, cx| s.focus(window, cx));
        search.read(cx).focus_handle(cx)
    });
    vcx.run_until_parked();
    assert!(
        vcx.update(|w, _| handle.is_focused(w)),
        "setup: the shortcut filter holds focus"
    );
    Fixture {
        app,
        vcx,
        cells,
        target,
    }
}

/// c1: a real Tab to a cell below the fold reveals the cell and its ring, with no
/// explicit draw after the key.
#[gpui::test]
fn c1_a_real_tab_to_a_below_fold_shortcut_cell_reveals_it(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    let action = f.cells[f.target].action.clone();
    assert!(
        tab_to(&mut f.vcx, &f.cells, &action),
        "FOCUS: Tab never reached the {action} cell"
    );
    let view = viewport(&f.app, &mut f.vcx);
    assert!(
        offset_y(&f.app, &mut f.vcx) < px(0.),
        "the page did not scroll to a cell that began below the fold"
    );
    let cell = f.vcx.debug_bounds(f.cells[f.target].cell).expect("drawn");
    let ring = f.vcx.debug_bounds(f.cells[f.target].ring).expect("mounted");
    assert!(
        within(cell, view),
        "the focused cell {cell:?} is not inside the viewport {view:?}"
    );
    assert!(
        within(ring, view),
        "its keyboard ring {ring:?} is not inside the viewport {view:?}"
    );
}

/// c2: on the revealed cell Enter still records for it, and neither starting nor
/// cancelling the recording moves the page.
#[gpui::test]
fn c2_enter_on_the_revealed_cell_records_without_moving_the_page(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    let action = f.cells[f.target].action.clone();
    assert!(
        tab_to(&mut f.vcx, &f.cells, &action),
        "FOCUS: Tab never reached the {action} cell"
    );
    settle(&mut f.vcx);
    let view = viewport(&f.app, &mut f.vcx);
    let at = offset_y(&f.app, &mut f.vcx);
    assert!(
        at < px(0.),
        "setup: the cell was revealed (offset {at:?}), so recording is tried on the revealed cell"
    );
    let cell = f.vcx.debug_bounds(f.cells[f.target].cell).expect("drawn");
    assert!(
        within(cell, view),
        "setup: the revealed cell {cell:?} is inside {view:?}"
    );

    press(&mut f.vcx, "enter");
    assert_eq!(
        recording(&f.app, &mut f.vcx).as_deref(),
        Some(action.as_str()),
        "Enter on the focused cell records for it"
    );
    assert_eq!(
        offset_y(&f.app, &mut f.vcx),
        at,
        "starting the recording moved the page"
    );
    press(&mut f.vcx, "escape");
    assert_eq!(
        recording(&f.app, &mut f.vcx),
        None,
        "Escape cancels the recording"
    );
    assert_eq!(
        offset_y(&f.app, &mut f.vcx),
        at,
        "cancelling the recording moved the page"
    );
}

/// c3: unmount ownership. With the revealed cell focused, leaving the page drops
/// the cell's handle and nothing scrolls the page afterwards. Only a weak handle
/// leaves the closure that reads the focus, so the test cannot keep the cell alive.
#[gpui::test]
fn c3_leaving_the_page_releases_the_focused_cell_and_scrolls_nothing(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    let action = f.cells[f.target].action.clone();
    assert!(
        tab_to(&mut f.vcx, &f.cells, &action),
        "FOCUS: Tab never reached the {action} cell"
    );
    let weak = f
        .vcx
        .update(|w, cx| w.focused(cx).map(|handle| handle.downgrade()))
        .expect("setup: a cell holds focus");
    assert!(
        weak.upgrade().is_some(),
        "setup: the cell's handle is alive with no strong reference in the test"
    );

    f.app.update_in(&mut f.vcx, |app, window, cx| {
        app.navigate_settings(SettingsSection::Appearance, None, window, cx);
    });
    settle(&mut f.vcx);
    assert!(
        f.vcx.debug_bounds(f.cells[f.target].cell).is_none(),
        "setup: the cell is no longer drawn"
    );
    assert!(
        weak.upgrade().is_none(),
        "the unmounted cell's focus handle was released"
    );
    let after = offset_y(&f.app, &mut f.vcx);
    settle(&mut f.vcx);
    settle(&mut f.vcx);
    assert_eq!(
        offset_y(&f.app, &mut f.vcx),
        after,
        "something scrolled the page after the cell was gone"
    );
}

// ------------------------------------------------------------------ the Reset link
//
// The self-removing Reset link of a modified row, tested ITSELF (the earlier
// d400 Reset control resets a cell that stays mounted). `reset-NewTab` is the
// stop key the link is expected to carry once it is a shared Stop, so the real
// Tab tests are RED until then; the click baseline passes on today's tree.

fn marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|w, _| {
        w.context_stack()
            .iter()
            .rev()
            .find_map(|c| c.get(super::kit::STOP_MARK).map(|v| v.to_string()))
    })
}

fn focused(vcx: &mut VisualTestContext) -> Option<gpui::FocusHandle> {
    vcx.update(|w, cx| w.focused(cx))
}

fn new_tab_is_overridden(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|_, cx| cx.global::<Config>().keybindings.contains_key("NewTab"))
}

/// Real Tabs (`run_until_parked` after each) until the marker is `id`, or the
/// focus stops moving to new handles.
fn tab_to_marker(vcx: &mut VisualTestContext, id: &str) -> bool {
    let mut seen = vec![focused(vcx)];
    for _ in 0..MAX_TABS {
        if marker(vcx).as_deref() == Some(id) {
            return true;
        }
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        let now = focused(vcx);
        if now.is_none() || seen.contains(&now) {
            return marker(vcx).as_deref() == Some(id);
        }
        seen.push(now);
    }
    marker(vcx).as_deref() == Some(id)
}

/// The Keyboard Shortcuts page in an ACTIVATED window tall enough that every row
/// is on screen, with New Tab carrying one override. The override is a LABELLED
/// CONFIG SETUP (not a UI path): it is what makes the row "modified" and draws
/// its Reset link.
fn reset_fixture(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext) {
    use crate::core::config::KeybindingOverride;
    use gpui::Focusable as _;
    let (app, mut vcx) = raw_harness(cx);
    vcx.update(|w, _| w.activate_window());
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| w.is_window_active()),
        "setup: the test window is active"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Keybindings, window, cx);
    });
    vcx.simulate_resize(size(px(1100.), px(4000.)));
    settle(&mut vcx);

    // SETUP (config): one override on New Tab, then a re-render.
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().keybindings.insert(
            "NewTab".to_string(),
            KeybindingOverride::Add("ctrl-alt-shift-n".to_string()),
        );
        crate::ui::keymap::rebind(cx);
    });
    app.update_in(&mut vcx, |_, _, cx| cx.notify());
    settle(&mut vcx);
    assert!(
        new_tab_is_overridden(&mut vcx),
        "setup: New Tab carries an override"
    );
    assert!(
        vcx.debug_bounds("reset-NewTab").is_some(),
        "setup: the modified row draws its Reset link"
    );
    assert!(
        vcx.debug_bounds("kb-NewTab").is_some(),
        "setup: and its shortcut cell"
    );

    let handle = app.update_in(&mut vcx, |app, window, cx| {
        let search = app
            .active_settings()
            .expect("settings open")
            .shortcut_search
            .clone();
        search.update(cx, |s, cx| s.focus(window, cx));
        search.read(cx).focus_handle(cx)
    });
    vcx.run_until_parked();
    assert!(
        vcx.update(|w, _| handle.is_focused(w)),
        "setup: the shortcut filter holds focus"
    );
    (app, vcx)
}

/// k1: the baseline. A pointer click on the Reset link removes the override and
/// the link, and leaves the row's cell. Passes on today's tree.
#[gpui::test]
fn k1_a_click_on_reset_removes_the_override_and_the_link(cx: &mut TestAppContext) {
    let (_app, mut vcx) = reset_fixture(cx);
    let at = vcx.debug_bounds("reset-NewTab").expect("drawn");
    vcx.simulate_click(at.center(), Modifiers::none());
    settle(&mut vcx);
    assert!(
        !new_tab_is_overridden(&mut vcx),
        "Reset removed New Tab's override"
    );
    assert!(
        vcx.debug_bounds("reset-NewTab").is_none(),
        "the row is no longer modified, so its Reset link is gone"
    );
    assert!(
        vcx.debug_bounds("kb-NewTab").is_some(),
        "the row's shortcut cell stays"
    );
}

/// k2: the keyboard path. A real Tab reaches the Reset link (marker
/// `reset-NewTab`), the focus handle is held weakly only, Enter resets, the link
/// and its handle go away, and the window's keyboard still works: a real Tab
/// lands on something mounted that holds focus, and Escape closes the page.
#[gpui::test]
fn k2_tab_to_reset_then_enter_removes_the_link_and_the_keyboard_survives(cx: &mut TestAppContext) {
    let (app, mut vcx) = reset_fixture(cx);
    assert!(
        tab_to_marker(&mut vcx, "reset-NewTab"),
        "FOCUS: Tab never reached the Reset link (marker reset-NewTab)"
    );
    let weak = vcx
        .update(|w, cx| w.focused(cx).map(|handle| handle.downgrade()))
        .expect("setup: the Reset link holds focus");
    assert!(
        weak.upgrade().is_some(),
        "setup: the link's handle is alive with no strong reference in the test"
    );

    press(&mut vcx, "enter");
    assert!(
        !new_tab_is_overridden(&mut vcx),
        "Enter on the focused Reset link removed New Tab's override"
    );
    assert!(
        vcx.debug_bounds("reset-NewTab").is_none(),
        "the Reset link left the page"
    );
    settle(&mut vcx);
    assert!(
        weak.upgrade().is_none(),
        "the removed link's focus handle was released"
    );

    press(&mut vcx, "tab");
    let (mounted, stack) = vcx.update(|w, _| {
        let stack = w.context_stack();
        (
            stack.iter().any(|c| c.contains("Root")),
            format!("{stack:?}"),
        )
    });
    assert!(
        mounted,
        "after Tab the window's focus is on nothing drawn; stack: {stack}"
    );
    let live_new_handle = vcx.update(|w, cx| {
        w.focused(cx)
            .is_some_and(|focused| weak != focused && focused.is_focused(w))
    });
    assert!(
        live_new_handle,
        "after Tab the focus is not a new handle that reports itself focused"
    );
    press(&mut vcx, "escape");
    assert!(
        !app.update_in(&mut vcx, |app, _, _| app.has_settings()),
        "Escape still closes the page after the link was removed"
    );
}
