//! The commit pill's inset focus ring (A2, decisions 6 and 7).
//!
//! While the frame is committable (inverted, filled with the panel's ink) each half takes the
//! toolkit's `no_focus_ring` (applied by `CommitHalf`, which owns the `Button`; the painted host below
//! has no opt-out of its own) and `CommitHalf` draws the ring just inside the half in the panel's own
//! colour. Not committable, the frame is a faint tint of the page and the halves keep the ordinary
//! outer gap ring. `CommitHalf` is mounted around both halves in every state.
//!
//! Three kinds of test, because they can see different things:
//!
//! * **Real panel** (`VisualTestContext`, the real `Tty59App`, a real repository): dispatch, focus and
//!   state. It exposes layout and read-only builder styles (`paint_probe`), NOT the painted scene, so no
//!   test here counts painted quads. The inset overlay's builder notes say which half shows its ring in
//!   a frame, in which colour and radii. Every frame that is inspected is an EXPLICIT `window.draw` after
//!   the notes were cleared (`run_until_parked` alone only drains the dispatcher); the first-frame test
//!   inspects the very first explicit draw after the change, with no repair frame behind it. Finding a
//!   half by `window.focus_next`/`focus_prev` is labelled SETUP SEARCH; the order and the key paths are
//!   then asserted with actually dispatched Tab/Shift+Tab and Enter/Space (key down AND key up, since
//!   `simulate_keystrokes` sends the down only). What the commit callback and the menu factory ran is
//!   counted by `commit_ring_events` (cfg(test), read-only, reset after the fixture is set up).
//! * **Component sweep** (all 877 themes, the real `apply_theme`): the inset ring against the actual
//!   `commit_paint(true)` fill and hover, at 3.0, named pairs, no formula copied.
//! * **Scene quads** (`HeadlessAppContext` with a quad recorder, copied from gpui-component's
//!   `button/focus_ring_tests.rs`; test-only, no runtime hook): a small host mounts the SAME
//!   `CommitHalf` around real toolkit `Button`s in the same frame composition and reads the painted
//!   `Scene::quads`. The headless window has a fixed scale factor of 2, so these run at 2x only and make
//!   no 1x claim; native 1x/2x captures of the real panel remain the proof.
//!
//! The ring's width is the named `COMMIT_RING_W`; it is checked against the width the toolkit's own outer
//! ring is painted at in the not-committable twin, not trusted. The self-contained recorder below is a
//! copy on purpose: this file does not depend on the global role's test file.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui::{
    AbsoluteLength, AnyWindowHandle, AppContext as _, Background, Context, DevicePixels, Entity,
    FocusHandle, HeadlessAppContext, Hsla, IntoElement, KeyUpEvent, Keystroke, NoopTextSystem,
    ParentElement as _, PlatformAtlas, PlatformHeadlessRenderer, PlatformInput, Quad, Render,
    Scene, Size, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px, rgb, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Disableable as _, WindowExt as _, h_flex};

use super::super::{
    COMMIT_CHEVRON_W, COMMIT_H, COMMIT_RADIUS, COMMIT_RING_W, CommitHalf, commit_half,
    commit_paint, commit_plan, commit_ring_events,
};
use super::{click, git, scm_panel_on, scratch, serial};
use crate::core::config::{Config, CoreConfig};
use crate::terminal::git_data::{index_of, status_of};
use crate::ui::app::Tty59App;
use crate::ui::paint_probe::{self, hsla24};
use crate::ui::presets::{self, Surfaces, contrast};
use crate::ui::rounding::{HAIRLINE, RoundedCorners as _, segment_corners};
use crate::ui::theme::apply_theme;

/// The notes `CommitHalf` leaves for the overlay it draws (read-only builder styles).
const LABEL_RING: &str = "scm-commit-ring.inset";
const CHEVRON_RING: &str = "scm-commit-menu-ring.inset";

// ---------------------------------------------------------------------------------------------
// Real panel: dispatch, focus, state
// ---------------------------------------------------------------------------------------------

type Panel = (
    Entity<Tty59App>,
    VisualTestContext,
    crate::daemon::transport::Stream,
);

/// The real panel on a real repository with one staged file, its status and index landed. Whether
/// the pill is committable is then a matter of the message alone. The event counters are zeroed
/// here: the fixture's own setup is never counted.
fn staged_repo_panel(cx: &mut TestAppContext, name: &str) -> Panel {
    crate::core::config::pin_test_config_dir();
    let root = scratch(name);
    git(&root, &["init", "--quiet"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&root, &["add", "a.txt"]);
    let panel = scm_panel_on(cx, &root, |app, cx| {
        app.scm.repo.as_ref().is_some_and(|r| {
            status_of(cx, r.host, &r.root).is_some_and(|s| s.staged().count() == 1)
                && index_of(cx, r.host, &r.root).is_some()
        })
    });
    commit_ring_events::reset();
    panel
}

/// Lets the app settle, forgets what earlier frames observed, and draws one explicit frame. The
/// notes that follow are exactly that frame's.
fn settle(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |_, _, cx| cx.notify());
    vcx.run_until_parked();
    draw_one_frame(vcx);
}

fn draw_one_frame(vcx: &mut VisualTestContext) {
    paint_probe::clear();
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

fn current_rings() -> (bool, bool) {
    (
        !paint_probe::all(LABEL_RING).is_empty(),
        !paint_probe::all(CHEVRON_RING).is_empty(),
    )
}

/// Which halves show their inset ring in a freshly drawn explicit frame: `(label half, chevron)`.
fn rings(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> (bool, bool) {
    settle(app, vcx);
    current_rings()
}

/// Applies `change` and then draws EXACTLY ONE explicit frame with nothing parked in between: the
/// rings of the first frame after the change, with no repair frame hidden behind it.
fn first_frame_after(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    change: impl FnOnce(&mut Tty59App, &mut Window, &mut Context<Tty59App>),
) -> (bool, bool) {
    app.update_in(vcx, change);
    draw_one_frame(vcx);
    current_rings()
}

/// SETUP SEARCH, not conformance: one tab stop forward or back through `window.focus_next`/`focus_prev`,
/// then the frame's rings. Used only to find a half; the order is asserted with real Tab below.
fn search_step(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, forward: bool) -> (bool, bool) {
    vcx.update(|window, cx| {
        if forward {
            window.focus_next(cx)
        } else {
            window.focus_prev(cx)
        }
    });
    rings(app, vcx)
}

/// SETUP SEARCH: forward until `want` shows; fails if it never does within `limit` stops.
fn search_to(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    want: (bool, bool),
    limit: usize,
) {
    for n in 1..=limit {
        let now = search_step(app, vcx, true);
        assert!(
            now != (true, true),
            "both halves show a ring at once (stop {n})"
        );
        if now == want {
            return;
        }
    }
    panic!("never reached {want:?} within {limit} tab stops");
}

/// A key as a keyboard delivers it: the down AND the up (`simulate_keystrokes` sends the down only),
/// then parked.
fn press(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    let keystroke = Keystroke::parse(key).unwrap();
    vcx.update(|window, cx| {
        window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
    });
    vcx.run_until_parked();
}

/// An actually dispatched Tab or Shift+Tab, then the frame's rings.
fn tab(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, forward: bool) -> (bool, bool) {
    press(vcx, if forward { "tab" } else { "shift-tab" });
    rings(app, vcx)
}

fn focused(vcx: &mut VisualTestContext) -> Option<FocusHandle> {
    vcx.update(|window, cx| window.focused(cx))
}

fn set_value(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, text: &'static str) {
    app.update_in(vcx, |app, window, cx| {
        let input = app
            .scm
            .commit_input
            .clone()
            .expect("the commit box was drawn");
        input.update(cx, |state, cx| state.set_value(text, window, cx));
    });
}

/// Sets the message and settles (setup).
fn set_message(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, text: &'static str) {
    set_value(app, vcx, text);
    settle(app, vcx);
}

/// Sets the message and inspects the exact first explicit draw after it.
fn set_message_first_frame(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    text: &'static str,
) -> (bool, bool) {
    first_frame_after(app, vcx, |app, window, cx| {
        let input = app
            .scm
            .commit_input
            .clone()
            .expect("the commit box was drawn");
        input.update(cx, |state, cx| state.set_value(text, window, cx));
    })
}

/// What the panel computes for the button, asked of the same function the panel asks.
fn is_committable(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
    app.update_in(vcx, |app, _, cx| {
        let repo = app.scm.repo.clone().expect("a repository is on screen");
        let status = status_of(cx, repo.host, &repo.root).expect("the status landed");
        let message = app
            .scm
            .commit_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        commit_plan(&status, app.scm.amend, &message).enabled
    })
}

fn armed(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
    app.update_in(vcx, |app, _, _| app.scm.committing.is_some())
}

fn toasts(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> usize {
    app.update_in(vcx, |_, window, cx| window.notifications(cx).len())
}

fn panel_ink(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> u32 {
    app.update_in(vcx, |_, _, cx| cx.global::<Surfaces>().sidebar.base)
}

/// The note for one half's overlay as a (colour, radii) pair, to compare with what the half is given.
fn corners_of(half: usize) -> [Option<AbsoluteLength>; 4] {
    let c = segment_corners(half, 2, COMMIT_RADIUS, HAIRLINE);
    [
        Some(AbsoluteLength::Pixels(c.top_left)),
        Some(AbsoluteLength::Pixels(c.top_right)),
        Some(AbsoluteLength::Pixels(c.bottom_right)),
        Some(AbsoluteLength::Pixels(c.bottom_left)),
    ]
}

/// Committable: both halves are tab stops in order, each shows exactly one inset ring in the panel's
/// colour and its own radii, and nothing between them or around them is a stop (so the always-mounted
/// ancestor is never one). The halves are FOUND by setup search; the order is then asserted with
/// actually dispatched Tab and Shift+Tab.
#[gpui::test]
fn live_halves_show_one_inset_ring_each_in_real_tab_order(cx: &mut TestAppContext) {
    let _serial = serial();
    let _probe = paint_probe::scope();
    let (app, mut vcx, _pane) = staged_repo_panel(cx, "commit-ring-order");
    set_message(&app, &mut vcx, "wip");
    assert!(
        is_committable(&app, &mut vcx),
        "a staged file and a message make the pill committable"
    );

    search_to(&app, &mut vcx, (true, false), 80);
    let ink = panel_ink(&app, &mut vcx);
    let note = paint_probe::all(LABEL_RING)
        .pop()
        .expect("the label half drew its ring");
    assert_eq!(
        note.border,
        Some(ink),
        "the inset ring is the panel's own colour"
    );
    assert_eq!(
        note.corners,
        corners_of(0),
        "the ring takes the half's own radii"
    );

    // From here on, real keys.
    assert_eq!(
        tab(&app, &mut vcx, true),
        (false, true),
        "Tab from the label half lands on the chevron, nothing in between"
    );
    let note = paint_probe::all(CHEVRON_RING)
        .pop()
        .expect("the chevron drew its ring");
    assert_eq!(note.border, Some(ink));
    assert_eq!(note.corners, corners_of(1));
    assert_eq!(
        tab(&app, &mut vcx, true),
        (false, false),
        "Tab from the chevron leaves the pill: the wrapper is no stop of its own"
    );
    assert_eq!(
        tab(&app, &mut vcx, false),
        (false, true),
        "Shift+Tab comes back to the chevron"
    );
    assert_eq!(
        tab(&app, &mut vcx, false),
        (true, false),
        "and to the label half"
    );
    assert_eq!(tab(&app, &mut vcx, false), (false, false));
}

/// Flip between committable and not, with the chevron focused (it is enabled in both states): the
/// focus survives, the inset ring is there exactly while committable, and it is on the exact first
/// explicit draw after the change, not on a repair frame behind it.
#[gpui::test]
fn the_chevron_keeps_focus_through_committable_flips(cx: &mut TestAppContext) {
    let _serial = serial();
    let _probe = paint_probe::scope();
    let (app, mut vcx, _pane) = staged_repo_panel(cx, "commit-ring-flip");
    set_message(&app, &mut vcx, "wip");
    search_to(&app, &mut vcx, (false, true), 80);
    let before = focused(&mut vcx);
    assert!(before.is_some(), "the chevron holds focus");

    for round in 0..2 {
        let first = set_message_first_frame(&app, &mut vcx, "");
        assert_eq!(
            first,
            (false, false),
            "round {round}: the first draw after the message went has no inset ring"
        );
        assert!(
            !is_committable(&app, &mut vcx),
            "round {round}: no message, not committable"
        );
        assert!(
            focused(&mut vcx) == before,
            "round {round}: focus moved when the pill sank back"
        );

        let first = set_message_first_frame(&app, &mut vcx, "wip");
        assert_eq!(
            first,
            (false, true),
            "round {round}: the inset ring is on the very first draw after the message came back"
        );
        assert!(
            is_committable(&app, &mut vcx),
            "round {round}: message back, committable"
        );
        assert!(
            focused(&mut vcx) == before,
            "round {round}: focus moved when the pill inverted"
        );
    }
}

/// A real mouse press on the chevron opens its menu once (the menu factory ran once; the label's
/// commit callback never), focus moves into the menu, and Escape (which the open popover binds) gives
/// focus, and the ring, back without opening it again.
///
/// This is the MOUSE witness: the toolkit's `Popover` toggles on a mouse-down (`popover.rs`). The
/// keyboard path (Enter or Space on the focused chevron, which a `DropdownMenu` trigger opens on key
/// release) has its own test below. The menu is built by the popover's
/// render once it is open, so the factory runs on the first DRAW after the press, not on the press:
/// the counter is read after one explicit frame. The ring while the menu is open is not asserted: the
/// popover content is a deferred child of the chevron's wrapper in the dispatch tree, so whether the
/// half keeps its ring then is unspecified here.
#[gpui::test]
fn a_mouse_press_on_the_chevron_opens_the_menu_once_and_escape_restores_focus_and_ring(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let _probe = paint_probe::scope();
    let (app, mut vcx, _pane) = staged_repo_panel(cx, "commit-ring-menu");
    set_message(&app, &mut vcx, "wip");
    search_to(&app, &mut vcx, (false, true), 80);
    let before = focused(&mut vcx);
    commit_ring_events::reset();

    // The chevron's centre from the label half's real bounds: its right edge, the seam, half the chevron.
    let label = vcx
        .debug_bounds("scm-commit")
        .expect("the label half is drawn");
    let at = point(
        label.origin.x + label.size.width + px(SEAM_W + COMMIT_CHEVRON_W / 2.),
        label.center().y,
    );
    click(&mut vcx, at);
    // One explicit frame: the open popover builds its menu while it renders.
    settle(&app, &mut vcx);
    assert_eq!(
        commit_ring_events::menu_opens(),
        1,
        "a press on the chevron must open the menu exactly once (the click must land on the chevron)"
    );
    assert!(
        focused(&mut vcx) != before,
        "focus is inside the opened menu"
    );
    let _ = rings(&app, &mut vcx);
    assert_eq!(
        commit_ring_events::menu_opens(),
        1,
        "a further frame must not open it again"
    );
    assert_eq!(commit_ring_events::commit_clicks(), 0);
    assert!(
        !armed(&app, &mut vcx) && toasts(&app, &mut vcx) == 0,
        "opening the menu commits nothing"
    );

    press(&mut vcx, "escape");
    assert_eq!(
        rings(&app, &mut vcx),
        (false, true),
        "Escape gives the ring back"
    );
    assert!(
        focused(&mut vcx) == before,
        "Escape gives focus back to the chevron"
    );
    assert_eq!(
        commit_ring_events::menu_opens(),
        1,
        "Escape reopened the menu"
    );
}

/// The KEYBOARD witness (needs the toolkit's keyboard-open `DropdownMenu`, 79a9176b): a real Tab from
/// the label half lands on the chevron (its inset ring on the first frame), Enter, key down and key up
/// as a keyboard delivers them, opens its menu exactly once (the menu factory runs on the first draw
/// after the key came up; the label's commit callback never runs), focus moves into the menu, and Escape
/// gives focus to the very same chevron and its inset ring back without opening the menu again.
///
/// The label half is only the setup search's landing point; the step under test, Tab onto the chevron,
/// is an actually dispatched Tab.
#[gpui::test]
fn enter_on_the_focused_chevron_opens_the_menu_once_and_escape_restores_focus_and_ring(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let _probe = paint_probe::scope();
    let (app, mut vcx, _pane) = staged_repo_panel(cx, "commit-ring-menu-key");
    set_message(&app, &mut vcx, "wip");
    search_to(&app, &mut vcx, (true, false), 80);

    // A real Tab: label half to chevron.
    assert_eq!(
        tab(&app, &mut vcx, true),
        (false, true),
        "a real Tab from the label half lands on the chevron and shows its inset ring"
    );
    let before = focused(&mut vcx);
    assert!(before.is_some(), "the chevron holds focus");
    commit_ring_events::reset();

    // Enter: key down, key up, then the first explicit frame (the open popover builds its menu while
    // it renders).
    press(&mut vcx, "enter");
    settle(&app, &mut vcx);
    assert_eq!(
        commit_ring_events::menu_opens(),
        1,
        "Enter on the focused chevron must open the menu exactly once"
    );
    assert!(
        focused(&mut vcx) != before,
        "focus is inside the opened menu"
    );
    let _ = rings(&app, &mut vcx);
    assert_eq!(
        commit_ring_events::menu_opens(),
        1,
        "a further frame must not open it again"
    );
    assert_eq!(
        commit_ring_events::commit_clicks(),
        0,
        "the chevron's Enter must not run the label half's commit"
    );
    assert!(
        !armed(&app, &mut vcx) && toasts(&app, &mut vcx) == 0,
        "opening the menu commits nothing"
    );

    press(&mut vcx, "escape");
    assert_eq!(
        rings(&app, &mut vcx),
        (false, true),
        "Escape gives the chevron's inset ring back"
    );
    assert!(
        focused(&mut vcx) == before,
        "Escape gives focus back to the very same chevron"
    );
    assert_eq!(
        commit_ring_events::menu_opens(),
        1,
        "Escape reopened the menu"
    );
    assert_eq!(commit_ring_events::commit_clicks(), 0);
    assert!(
        !armed(&app, &mut vcx) && toasts(&app, &mut vcx) == 0,
        "nothing was committed"
    );
}

/// Enter and Space on the focused label half run its action exactly once (key down and up).
fn activate_label_half_with(cx: &mut TestAppContext, name: &str, key: &str) {
    let _serial = serial();
    let _probe = paint_probe::scope();
    let (app, mut vcx, _pane) = staged_repo_panel(cx, name);
    set_message(&app, &mut vcx, "wip");
    search_to(&app, &mut vcx, (true, false), 80);
    commit_ring_events::reset();

    press(&mut vcx, key);
    assert_eq!(
        commit_ring_events::commit_clicks(),
        1,
        "{key} on the focused label half must run the commit callback exactly once (the real Button must still activate, not twice either)"
    );
    assert_eq!(commit_ring_events::menu_opens(), 0);
    assert!(armed(&app, &mut vcx), "{key}: the commit was dispatched");
    assert_eq!(
        toasts(&app, &mut vcx),
        0,
        "{key}: a commit that ran says nothing"
    );
}

#[gpui::test]
fn enter_on_the_focused_label_half_commits_once(cx: &mut TestAppContext) {
    activate_label_half_with(cx, "commit-ring-enter", "enter");
}

#[gpui::test]
fn space_on_the_focused_label_half_commits_once(cx: &mut TestAppContext) {
    activate_label_half_with(cx, "commit-ring-space", "space");
}

/// A mouse press on the committable label half commits once and leaves no ring (the pointer does not
/// focus a button), exactly as before.
#[gpui::test]
fn a_mouse_press_on_the_live_label_half_commits_once_and_draws_no_ring(cx: &mut TestAppContext) {
    let _serial = serial();
    let _probe = paint_probe::scope();
    let (app, mut vcx, _pane) = staged_repo_panel(cx, "commit-ring-mouse");
    set_message(&app, &mut vcx, "wip");
    let before = focused(&mut vcx);
    commit_ring_events::reset();
    let at = vcx
        .debug_bounds("scm-commit")
        .expect("the label half is drawn")
        .center();
    click(&mut vcx, at);
    assert_eq!(
        commit_ring_events::commit_clicks(),
        1,
        "the click must reach the real Button and run the commit callback exactly once"
    );
    assert!(armed(&app, &mut vcx), "the commit was dispatched");
    assert_eq!(toasts(&app, &mut vcx), 0);
    assert_eq!(
        rings(&app, &mut vcx),
        (false, false),
        "a press does not focus the half"
    );
    assert!(
        focused(&mut vcx) == before,
        "a press does not move focus onto the wrapper"
    );
}

/// The overlay is decorative: a press on the 1.5px ring edge of the focused half still reaches the
/// button underneath, once.
#[gpui::test]
fn the_ring_edge_does_not_intercept_the_pointer(cx: &mut TestAppContext) {
    let _serial = serial();
    let _probe = paint_probe::scope();
    let (app, mut vcx, _pane) = staged_repo_panel(cx, "commit-ring-edge");
    set_message(&app, &mut vcx, "wip");
    search_to(&app, &mut vcx, (true, false), 80);
    commit_ring_events::reset();
    let bounds = vcx
        .debug_bounds("scm-commit")
        .expect("the label half is drawn");
    let edge = bounds.origin + point(COMMIT_RING_W / 2., bounds.size.height / 2.);
    click(&mut vcx, edge);
    assert_eq!(
        commit_ring_events::commit_clicks(),
        1,
        "a press on the ring's own pixels must reach the button exactly once"
    );
    assert_eq!(toasts(&app, &mut vcx), 0);
}

/// Not committable, the label half is disabled: it is no tab stop, a press or Enter does nothing (the
/// commit callback never runs, the menu is never built), and the wrapper never takes focus from a
/// press.
#[gpui::test]
fn the_disabled_label_half_skips_focus_and_dispatches_nothing(cx: &mut TestAppContext) {
    let _serial = serial();
    let _probe = paint_probe::scope();
    let (app, mut vcx, _pane) = staged_repo_panel(cx, "commit-ring-dead");
    set_message(&app, &mut vcx, "");
    assert!(!is_committable(&app, &mut vcx));
    let before = focused(&mut vcx);
    commit_ring_events::reset();

    let at = vcx
        .debug_bounds("scm-commit")
        .expect("the label half is drawn")
        .center();
    click(&mut vcx, at);
    assert_eq!(
        commit_ring_events::commit_clicks(),
        0,
        "a dead button ran the commit callback"
    );
    assert!(
        !armed(&app, &mut vcx),
        "a dead button still dispatched the commit"
    );
    assert_eq!(
        toasts(&app, &mut vcx),
        0,
        "a swallowed press should say nothing"
    );
    assert!(
        focused(&mut vcx) == before,
        "a press on the disabled half moved focus (onto the wrapper?)"
    );

    // Walk every stop of the panel (setup search): the label half is never one, and no inset ring shows.
    for n in 1..=80 {
        assert_eq!(
            search_step(&app, &mut vcx, true),
            (false, false),
            "stop {n}: an inset ring while not committable"
        );
    }
    press(&mut vcx, "enter");
    assert_eq!(
        commit_ring_events::commit_clicks(),
        0,
        "Enter somewhere in the panel ran the commit callback while nothing is committable"
    );
    assert_eq!(commit_ring_events::menu_opens(), 0);
    assert!(!armed(&app, &mut vcx));
}

// ---------------------------------------------------------------------------------------------
// Component sweep: the inset ring against what the committable pill actually paints
// ---------------------------------------------------------------------------------------------

/// The inset ring is the panel's colour on the frame, which committable is filled with the theme's
/// ink and, under the pointer, with its hover blend. Measured, in every theme the app derives chrome
/// for, from the real `apply_theme` and the real `commit_paint(true)`, at 3.0 (a component, not
/// text). A miss is reported with its theme and pair; nothing falls back.
#[gpui::test]
fn the_inset_ring_clears_the_committable_frame_fill_and_hover_in_all_877_themes(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    let mut themes = presets::builtins();
    themes.extend(presets::generated_theme_grid());
    assert_eq!(themes.len(), 877, "13 built-ins and 864 generated palettes");
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(presets::Themes(themes.clone()));
        let mut misses = Vec::new();
        for theme in &themes {
            cx.set_global(Config(CoreConfig {
                theme_follow_system: false,
                theme_preset: theme.id.clone(),
                ..Default::default()
            }));
            apply_theme(None, cx);
            let paint = commit_paint(true, cx);
            let ring = hsla24(paint.ink);
            for (pair, other) in [
                ("committable frame fill", hsla24(paint.fill)),
                ("committable frame hover", hsla24(paint.hover)),
            ] {
                let ratio = contrast(ring, other);
                if ratio < 3.0 {
                    misses.push(format!(
                        "{}: inset ring {ring:06x} on {pair} {other:06x} is only {ratio:.2}:1",
                        theme.id
                    ));
                }
            }
        }
        assert!(
            misses.is_empty(),
            "inset ring vs the actual committable fill and hover: {} misses, first 12:\n{}",
            misses.len(),
            misses
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    });
}

// ---------------------------------------------------------------------------------------------
// Scene quads: the same CommitHalf around real Buttons, painted into a recorder
// ---------------------------------------------------------------------------------------------

/// An atlas that stores nothing: the buttons under test draw no glyphs.
struct NoAtlas;

impl PlatformAtlas for NoAtlas {
    fn get_or_insert_with<'a>(
        &self,
        _key: &gpui::AtlasKey,
        _build: &mut dyn FnMut() -> anyhow::Result<
            Option<(Size<DevicePixels>, std::borrow::Cow<'a, [u8]>)>,
        >,
    ) -> anyhow::Result<Option<gpui::AtlasTile>> {
        Ok(None)
    }

    fn remove(&self, _key: &gpui::AtlasKey) {}
}

/// Records the quads of the most recent scene instead of rendering it.
struct QuadRecorder {
    quads: Arc<Mutex<Vec<Quad>>>,
}

impl QuadRecorder {
    /// Fails when the log is poisoned: a stale scene must never be read back as the frame just drawn.
    fn record(&self, scene: &Scene) -> anyhow::Result<()> {
        let mut quads = self
            .quads
            .lock()
            .map_err(|_| anyhow::anyhow!("quad log mutex poisoned"))?;
        *quads = scene.quads.clone();
        Ok(())
    }
}

impl PlatformHeadlessRenderer for QuadRecorder {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        _size: Size<DevicePixels>,
    ) -> anyhow::Result<image::RgbaImage> {
        self.record(scene)?;
        Ok(image::RgbaImage::new(1, 1))
    }

    fn render_scene(&mut self, scene: &Scene, _size: Size<DevicePixels>) -> anyhow::Result<()> {
        self.record(scene)
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        Arc::new(NoAtlas)
    }
}

const PAD: f32 = 20.;
/// The label half's width in the host (a real label would need glyph metrics the headless text system
/// does not have), and the seam between the halves, as in the panel.
const LEFT_W: f32 = 60.;
const SEAM_W: f32 = 0.5;
/// The toolkit ring's offset the theme installs, and its stroke.
const OUTER_GAP: f32 = 2.;

/// The pill as the panel composes it, around real `Button`s, with the same `CommitHalf`.
struct PillHost {
    committable: Rc<Cell<bool>>,
}

impl Render for PillHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let live = self.committable.get();
        let paint = commit_paint(live, cx);
        let left = segment_corners(0, 2, COMMIT_RADIUS, HAIRLINE);
        let right = segment_corners(1, 2, COMMIT_RADIUS, HAIRLINE);
        div().size_full().p(px(PAD)).child(
            h_flex()
                .flex_none()
                .items_center()
                // The frame is a block child of the host, so without a width it would stretch to the host's.
                // It is constrained to its three parts, so the layout the checks use is the frame's own.
                .w(px(LEFT_W + SEAM_W + COMMIT_CHEVRON_W))
                .h(px(COMMIT_H))
                .rounded(COMMIT_RADIUS)
                .bg(paint.fill)
                .child(CommitHalf::new(
                    "scm-commit-ring",
                    live.then_some(paint.ink),
                    left,
                    Button::new("scm-commit")
                        .custom(commit_half(paint.ink, cx))
                        .w(px(LEFT_W))
                        .h_full()
                        .rounded_corners(left)
                        .disabled(!live),
                ))
                .child(
                    div()
                        .flex_none()
                        .w(px(SEAM_W))
                        .h(px(COMMIT_H - 12.))
                        .bg(paint.seam),
                )
                .child(CommitHalf::new(
                    "scm-commit-menu-ring",
                    live.then_some(paint.ink),
                    right,
                    Button::new("scm-commit-menu")
                        .custom(commit_half(paint.ink, cx))
                        .w(px(COMMIT_CHEVRON_W))
                        .h_full()
                        .rounded_corners(right),
                )),
        )
    }
}

struct Harness {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    quads: Arc<Mutex<Vec<Quad>>>,
    committable: Rc<Cell<bool>>,
}

impl Harness {
    /// A headless window with `theme` installed through the real `apply_theme`.
    fn open(theme: &presets::Theme) -> Self {
        let quads = Arc::new(Mutex::new(Vec::new()));
        let recorder_quads = quads.clone();
        let mut cx = HeadlessAppContext::with_platform(
            Arc::new(NoopTextSystem::new()),
            Arc::new(()),
            move || {
                Some(Box::new(QuadRecorder {
                    quads: recorder_quads.clone(),
                }) as Box<dyn PlatformHeadlessRenderer>)
            },
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(presets::Themes(vec![theme.clone()]));
            cx.set_global(Config(CoreConfig {
                theme_follow_system: false,
                theme_preset: theme.id.clone(),
                ..Default::default()
            }));
            apply_theme(None, cx);
        });
        let committable = Rc::new(Cell::new(false));
        let host_flag = committable.clone();
        let window = cx
            .open_window(size(px(200.), px(100.)), move |_, cx| {
                cx.new(|_| PillHost {
                    committable: host_flag,
                })
            })
            .unwrap();
        let mut harness = Self {
            cx,
            window: window.into(),
            quads,
            committable,
        };
        // First frame: registers the tab stops.
        harness.draw();
        harness
    }

    fn draw(&mut self) -> Vec<Quad> {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.draw(cx).clear();
                window.render_to_image().map(|_| ())
            })
            .and_then(|drawn| drawn)
            .unwrap();
        self.quads.lock().unwrap().clone()
    }

    fn focus(&mut self, forward: bool) {
        let moved = self
            .cx
            .update_window(self.window, |_, window, cx| {
                if forward {
                    window.focus_next(cx)
                } else {
                    window.focus_prev(cx)
                }
                window.focused(cx).is_some()
            })
            .unwrap();
        assert!(moved, "tab traversal left nothing focused");
    }

    fn has_focus(&mut self) -> bool {
        self.cx
            .update_window(self.window, |_, window, cx| window.focused(cx).is_some())
            .unwrap()
    }

    fn scale_factor(&mut self) -> f32 {
        self.cx
            .update_window(self.window, |_, window, _| window.scale_factor())
            .unwrap()
    }
}

type Edges4 = [f32; 4];

/// `[left, top, right, bottom]` of a quad's bounds, in device pixels.
fn edges(quad: &Quad) -> Edges4 {
    let b = quad.bounds;
    [
        b.origin.x.0,
        b.origin.y.0,
        b.origin.x.0 + b.size.width.0,
        b.origin.y.0 + b.size.height.0,
    ]
}

fn outset(e: Edges4, by: f32) -> Edges4 {
    [e[0] - by, e[1] - by, e[2] + by, e[3] + by]
}

fn near(a: Edges4, b: Edges4) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 0.01)
}

fn border_width(quad: &Quad) -> f32 {
    quad.border_widths.top.0
}

fn same_colour(a: Hsla, b: Hsla) -> bool {
    [(a.h, b.h), (a.s, b.s), (a.l, b.l), (a.a, b.a)]
        .iter()
        .all(|(x, y)| (x - y).abs() < 1e-4)
}

/// Where the pill's pieces are, in device pixels.
struct Layout {
    scale: f32,
    left: Edges4,
    right: Edges4,
    frame: Edges4,
}

impl Layout {
    fn new(scale: f32) -> Self {
        let top = PAD * scale;
        let bottom = (PAD + COMMIT_H) * scale;
        let left = [PAD * scale, top, (PAD + LEFT_W) * scale, bottom];
        let right = [
            (PAD + LEFT_W + SEAM_W) * scale,
            top,
            (PAD + LEFT_W + SEAM_W + COMMIT_CHEVRON_W) * scale,
            bottom,
        ];
        let frame = [left[0], top, right[2], bottom];
        Self {
            scale,
            left,
            right,
            frame,
        }
    }

    /// The painted focus indicators of a frame, named by where they sit: border-only quads that are
    /// neither the frame nor a button. An inset ring is exactly a half's own bounds; the toolkit's outer
    /// ring is the half's bounds outset by its gap plus its stroke.
    fn indicators(&self, quads: &[Quad]) -> Vec<(String, Quad)> {
        let outer = (OUTER_GAP + COMMIT_RING_W.as_f32()) * self.scale;
        quads
            .iter()
            .filter(|q| {
                let w = q.border_widths;
                [w.top, w.right, w.bottom, w.left].iter().all(|v| v.0 > 0.)
                    && !near(edges(q), self.frame)
            })
            .map(|q| {
                let e = edges(q);
                let name = if near(e, self.left) {
                    "inset-label"
                } else if near(e, self.right) {
                    "inset-chevron"
                } else if near(e, outset(self.left, outer)) {
                    "outer-label"
                } else if near(e, outset(self.right, outer)) {
                    "outer-chevron"
                } else {
                    "other"
                };
                (format!("{name} {e:?}"), *q)
            })
            .collect()
    }
}

fn names(indicators: &[(String, Quad)]) -> Vec<&str> {
    indicators
        .iter()
        .map(|(name, _)| name.split(' ').next().unwrap_or(""))
        .collect()
}

/// The painted pill, in the stock dark and light themes and in latte, dawn and one_dark_pro. Chevron focused: not committable it wears the
/// toolkit's outer ring; committable it wears one inset ring in the panel's colour inside its own
/// bounds, at the width the outer ring was painted at, at least 3:1 against the frame fill actually
/// painted; the label half's ring likewise; and every flip while focused shows exactly one indicator of
/// the right kind in that frame, with focus kept.
#[test]
fn the_painted_pill_shows_exactly_one_indicator_of_the_right_kind_through_every_flip() {
    crate::core::config::pin_test_config_dir();
    // The stock pair, and the three themes whose frame the earlier feasibility proof flagged.
    for id in [
        "dark",
        "light",
        "catppuccin_latte",
        "rose_pine_dawn",
        "one_dark_pro",
    ] {
        let theme = presets::builtins()
            .into_iter()
            .find(|theme| theme.id == id)
            .expect("a stock theme");
        let mut harness = Harness::open(&theme);
        let layout = Layout::new(harness.scale_factor());
        let s = layout.scale;
        let (panel, frame_fill) = harness.cx.update(|cx| {
            (
                cx.global::<Surfaces>().sidebar.base,
                commit_paint(true, cx).fill,
            )
        });
        let want_frame = format!("{:?}", Background::from(frame_fill));

        // Not committable: the label half is disabled, so the chevron is the only stop.
        harness.committable.set(false);
        harness.draw();
        harness.focus(true);
        let quads = harness.draw();
        let found = layout.indicators(&quads);
        assert_eq!(
            names(&found),
            ["outer-chevron"],
            "{id}: not committable, one outer ring on the chevron: {found:?}"
        );
        let outer_width = border_width(&found[0].1);
        assert!(
            (outer_width - COMMIT_RING_W.as_f32() * s).abs() < 0.01,
            "{id}: the toolkit's outer ring is painted {outer_width} device px wide, COMMIT_RING_W is {}",
            COMMIT_RING_W.as_f32() * s
        );

        // Committable, same focus: one inset ring on the chevron and no outer one.
        harness.committable.set(true);
        let quads = harness.draw();
        assert!(
            harness.has_focus(),
            "{id}: focus lost when the pill inverted"
        );
        let found = layout.indicators(&quads);
        assert_eq!(
            names(&found),
            ["inset-chevron"],
            "{id}: committable, one inset ring: {found:?}"
        );
        let ring = found[0].1;
        assert!(
            (border_width(&ring) - outer_width).abs() < 0.01,
            "{id}: the inset ring is {} wide, the outer twin {outer_width}",
            border_width(&ring)
        );
        assert!(
            same_colour(ring.border_color, Hsla::from(rgb(panel))),
            "{id}: the inset ring is {:?}, not the panel colour {panel:06x}",
            ring.border_color
        );
        // Radii of the chevron: square on the seam side, the frame's inner radius outside.
        let r = segment_corners(1, 2, COMMIT_RADIUS, HAIRLINE);
        let got = [
            ring.corner_radii.top_left.0,
            ring.corner_radii.top_right.0,
            ring.corner_radii.bottom_right.0,
            ring.corner_radii.bottom_left.0,
        ];
        let want = [
            r.top_left.as_f32() * s,
            r.top_right.as_f32() * s,
            r.bottom_right.as_f32() * s,
            r.bottom_left.as_f32() * s,
        ];
        assert!(
            got.iter()
                .zip(want.iter())
                .all(|(g, w)| (g - w).abs() < 0.01),
            "{id}: the chevron ring's radii are {got:?}, the half's are {want:?}"
        );
        // The pair that matters: the ring as painted against the frame fill as painted.
        let frames: Vec<&Quad> = quads
            .iter()
            .filter(|q| near(edges(q), layout.frame) && format!("{:?}", q.background) == want_frame)
            .collect();
        assert_eq!(
            frames.len(),
            1,
            "{id}: the committable frame fill is painted once"
        );
        let ratio = contrast(hsla24(ring.border_color), hsla24(frame_fill));
        assert!(
            ratio >= 3.0,
            "{id}: the inset ring is only {ratio:.2}:1 on the painted frame fill"
        );

        // Back to the label half: its own inset ring.
        harness.focus(false);
        let quads = harness.draw();
        let found = layout.indicators(&quads);
        assert_eq!(
            names(&found),
            ["inset-label"],
            "{id}: the label half's one inset ring: {found:?}"
        );
        let r = segment_corners(0, 2, COMMIT_RADIUS, HAIRLINE);
        let ring = found[0].1;
        let got = [
            ring.corner_radii.top_left.0,
            ring.corner_radii.top_right.0,
            ring.corner_radii.bottom_right.0,
            ring.corner_radii.bottom_left.0,
        ];
        let want = [
            r.top_left.as_f32() * s,
            r.top_right.as_f32() * s,
            r.bottom_right.as_f32() * s,
            r.bottom_left.as_f32() * s,
        ];
        assert!(
            got.iter()
                .zip(want.iter())
                .all(|(g, w)| (g - w).abs() < 0.01),
            "{id}: the label ring's radii are {got:?}, the half's are {want:?}"
        );

        // Forward to the chevron again, then sink back to not committable while it holds focus.
        harness.focus(true);
        let found = layout.indicators(&harness.draw());
        assert_eq!(names(&found), ["inset-chevron"], "{id}: {found:?}");
        harness.committable.set(false);
        let found = layout.indicators(&harness.draw());
        assert!(
            harness.has_focus(),
            "{id}: focus lost when the pill sank back"
        );
        assert_eq!(
            names(&found),
            ["outer-chevron"],
            "{id}: back to the toolkit's outer ring on the very first frame: {found:?}"
        );
        assert!(
            (border_width(&found[0].1) - outer_width).abs() < 0.01,
            "{id}: the outer ring changed width after a round trip"
        );
    }
}
