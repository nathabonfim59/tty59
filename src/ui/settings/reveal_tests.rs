//! Controls for the keyboard reveal: a stop that opted into scrolling is brought
//! into view when a real Tab / Shift-Tab reaches it.
//!
//! * pure: the plan the reveal follows (`kit::reveal_delta` / `reveal_plan`),
//!   including the EFFECTIVE-movement rule for nested scrollers;
//! * real pages in an ACTIVATED narrow window (the reveal is a focus-in
//!   listener, and an inactive test window delivers no focus events):
//!   v1c reaches a control below the fold with real Tab and `run_until_parked`
//!   ONLY, no explicit draw, so a missing deferred redraw cannot hide; v4-v9 and
//!   the close/re-open control pin minimality, direction, pointer, pinned stops,
//!   the results page, the nested host list, and that a closed and re-opened
//!   page's stops move the NEW scroller and never a stale one.

use gpui::{
    Bounds, KeyUpEvent, Keystroke, Modifiers, Pixels, TestAppContext, VisualTestContext, point, px,
    size,
};

use super::kit::test_support::harness as raw_harness;
use super::kit::{Level, reveal_delta, reveal_plan};
use super::*;

const MAX_TABS: usize = 160;
const NARROW: (f32, f32) = (960., 420.);
const TARGET: &str = "window-opacity";

fn bounds(top: f32, bottom: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(0.), px(top)),
        size: size(px(100.), px(bottom - top)),
    }
}

// ------------------------------------------------------------------ pure plan

#[test]
fn d1_a_rect_below_the_view_moves_the_content_up_by_the_least() {
    let d = reveal_delta(bounds(0., 100.), bounds(120., 140.), px(0.), px(500.));
    assert_eq!(d, px(-40.), "its bottom to the view's bottom");
}

#[test]
fn d2_a_rect_above_the_view_moves_the_content_down_by_the_least() {
    let d = reveal_delta(bounds(100., 200.), bounds(80., 100.), px(-50.), px(500.));
    assert_eq!(d, px(20.), "its top to the view's top");
}

#[test]
fn d3_a_rect_already_inside_moves_nothing() {
    let d = reveal_delta(bounds(0., 100.), bounds(20., 40.), px(-30.), px(500.));
    assert_eq!(d, px(0.));
}

#[test]
fn d4_the_movement_is_limited_to_what_the_scroller_can_do() {
    // Needs -40 but only 10 is left to scroll: the effective movement is -10.
    let d = reveal_delta(bounds(0., 100.), bounds(120., 140.), px(-490.), px(500.));
    assert_eq!(d, px(-10.), "limited by max_offset");
    // Needs +20 up but the offset is already at the top.
    let d = reveal_delta(bounds(100., 200.), bounds(80., 100.), px(0.), px(500.));
    assert_eq!(d, px(0.), "limited by the top");
}

#[test]
fn d5_a_rect_taller_than_the_view_aligns_its_top() {
    let d = reveal_delta(bounds(0., 100.), bounds(20., 180.), px(0.), px(500.));
    assert_eq!(d, px(-20.), "top to top");
}

#[test]
fn d6_nested_scrollers_carry_the_rect_by_the_effective_movement_only() {
    // The inner list can move only 10 of the 50 the rect needs; the outer one is
    // planned from where the rect really ends up (bottom 140, outer view ends at
    // 130), not from where an unclamped move would have put it (inside it).
    let inner = Level {
        view: bounds(0., 100.),
        offset_y: px(0.),
        max_y: px(10.),
    };
    let outer = Level {
        view: bounds(0., 130.),
        offset_y: px(0.),
        max_y: px(500.),
    };
    assert_eq!(
        reveal_plan(bounds(130., 150.), &[inner, outer]),
        vec![px(-10.), px(-10.)],
        "inner moved what it could, outer picked up the rest"
    );
}

#[test]
fn d7_an_unpainted_inner_scroller_does_not_veto_the_outer_one() {
    let unpainted = Level {
        view: bounds(0., 0.),
        offset_y: px(0.),
        max_y: px(0.),
    };
    let outer = Level {
        view: bounds(0., 100.),
        offset_y: px(0.),
        max_y: px(500.),
    };
    assert_eq!(
        reveal_plan(bounds(120., 140.), &[unpainted, outer]),
        vec![px(0.), px(-40.)]
    );
}

// ------------------------------------------------------------------ helpers

fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..3 {
        vcx.run_until_parked();
        vcx.update(|w, cx| {
            let _ = w.draw(cx);
        });
    }
}

/// A real key, no explicit draw: the key down and its effects run to parked.
fn key_parked(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    vcx.run_until_parked();
}

fn key_settled(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    settle(vcx);
}

fn press(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    vcx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).expect("a key name"),
    });
    settle(vcx);
}

fn marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|w, _| {
        w.context_stack()
            .iter()
            .rev()
            .find_map(|c| c.get(kit::STOP_MARK).map(|v| v.to_string()))
    })
}

fn focused(vcx: &mut VisualTestContext) -> Option<gpui::FocusHandle> {
    vcx.update(|w, cx| w.focused(cx))
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn state<T>(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    f: impl FnOnce(&SettingsState) -> T,
) -> T {
    app.update_in(vcx, |app, _, _| {
        f(app.active_settings().expect("settings open"))
    })
}

fn viewport(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Bounds<Pixels> {
    state(app, vcx, |s| s.content_scroll.bounds())
}

fn offset_y(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Pixels {
    state(app, vcx, |s| s.content_scroll.offset().y)
}

fn within(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let slack = px(0.5);
    inner.top() >= outer.top() - slack
        && inner.bottom() <= outer.bottom() + slack
        && inner.left() >= outer.left() - slack
        && inner.right() <= outer.right() + slack
}

fn gap(vcx: &mut VisualTestContext) -> Pixels {
    vcx.update(|_, cx| {
        use gpui_component::ActiveTheme as _;
        cx.theme().focus_ring_offset()
    })
}

/// The app's window, ACTIVE (the reveal listens to focus-in, which an inactive
/// test window never delivers), the page opened and laid out narrow.
fn narrow(
    cx: &mut TestAppContext,
    section: SettingsSection,
    height: f32,
) -> (Entity<Tty59App>, VisualTestContext) {
    let (app, mut vcx) = raw_harness(cx);
    vcx.update(|w, _| w.activate_window());
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| w.is_window_active()),
        "setup: the test window is active"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(section, window, cx);
    });
    vcx.simulate_resize(size(px(NARROW.0), px(height)));
    settle(&mut vcx);
    (app, vcx)
}

fn focus_search(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    use gpui::Focusable as _;
    let handle = app.update_in(vcx, |app, window, cx| {
        let search = app.active_settings().expect("settings open").search.clone();
        search.update(cx, |s, cx| s.focus(window, cx));
        search.read(cx).focus_handle(cx)
    });
    vcx.run_until_parked();
    assert!(
        vcx.update(|w, _| handle.is_focused(w)),
        "setup: the search Input holds focus"
    );
}

/// Real Tabs (or Shift-Tabs) until the marker is `id`.
fn walk_to(vcx: &mut VisualTestContext, id: &str, key: &str, parked_only: bool) -> bool {
    let mut seen = vec![focused(vcx)];
    for _ in 0..MAX_TABS {
        if marker(vcx).as_deref() == Some(id) {
            return true;
        }
        if parked_only {
            key_parked(vcx, key);
        } else {
            key_settled(vcx, key);
        }
        let now = focused(vcx);
        if now.is_none() || seen.contains(&now) {
            return marker(vcx).as_deref() == Some(id);
        }
        seen.push(now);
    }
    false
}

// ------------------------------------------------------------------ v1c

/// v1c: the SAME reach as V1, but only real Tabs and `run_until_parked`: no
/// explicit draw anywhere after the page is laid out. The reveal moves the
/// scroller from a focus-in listener in the draw's focus phase, where a redraw
/// cannot be requested; if its deferred refresh were missing, the offset would
/// change with nothing drawn and the control would still be reported off-screen.
#[gpui::test]
fn v1c_a_real_tab_reveals_with_no_explicit_draw(cx: &mut TestAppContext) {
    let (app, mut vcx) = narrow(cx, SettingsSection::Appearance, NARROW.1);
    let view = viewport(&app, &mut vcx);
    let before = vcx.debug_bounds(TARGET).expect("laid out");
    assert!(
        before.top() > view.bottom(),
        "setup: starts below the viewport"
    );

    focus_search(&app, &mut vcx);
    assert!(
        walk_to(&mut vcx, TARGET, "tab", true),
        "FOCUS: Tab never reached {TARGET}"
    );
    let after = vcx.debug_bounds(TARGET).expect("still laid out");
    assert!(
        offset_y(&app, &mut vcx) < px(0.),
        "the page scrolled down to the focused control"
    );
    assert!(
        within(after, view),
        "VISIBLE with no explicit draw: {after:?} inside {view:?}"
    );
}

// ------------------------------------------------------------------ v4-v9

/// v4: the least movement: the control (and its ring's reach) ends at the
/// viewport's bottom edge, unless the page ran out of scroll, and two stops that
/// are already on screen cost no scrolling.
#[gpui::test]
fn v4_the_reveal_is_the_minimal_movement_and_visible_stops_do_not_scroll(cx: &mut TestAppContext) {
    let (app, mut vcx) = narrow(cx, SettingsSection::Appearance, NARROW.1);
    focus_search(&app, &mut vcx);
    assert!(walk_to(&mut vcx, TARGET, "tab", false), "FOCUS: reach");
    let view = viewport(&app, &mut vcx);
    let control = vcx.debug_bounds(TARGET).expect("drawn");
    let outset = gap(&mut vcx) + crate::ui::theme::COMMIT_RING_W;
    let slack = view.bottom() - (control.bottom() + outset);
    let max = state(&app, &mut vcx, |s| s.content_scroll.max_offset().y);
    assert!(
        slack.abs() <= px(0.5) || offset_y(&app, &mut vcx) <= -max + px(0.5),
        "the control's bottom plus the ring's reach ({outset:?}) meets the viewport's bottom \
         (slack {slack:?}) unless the page is at its end"
    );
    assert!(
        offset_y(&app, &mut vcx) < px(0.),
        "and the page really moved"
    );
}

/// v4b: Tab between two stops that are ALREADY on screen costs no scrolling. The
/// pair is the page's first two stops, the adjacent theme cards Light and Dark,
/// and "already on screen" is proved from their own bounds BEFORE any focus
/// moves, with the page at its top; exactly one Tab separates them.
#[gpui::test]
fn v4b_tab_between_two_visible_stops_does_not_scroll(cx: &mut TestAppContext) {
    let (app, mut vcx) = narrow(cx, SettingsSection::Appearance, 900.);
    let view = viewport(&app, &mut vcx);
    assert!(
        view.size.height > px(300.),
        "setup: the page is laid out ({view:?})"
    );
    assert_eq!(
        offset_y(&app, &mut vcx),
        px(0.),
        "setup: the page is at its top"
    );
    let light = vcx.debug_bounds("theme-mode-Light").expect("drawn");
    let dark = vcx.debug_bounds("theme-mode-Dark").expect("drawn");
    assert!(
        within(light, view) && within(dark, view),
        "setup, before any focus moves: both cards are on screen at offset 0 \
         (Light {light:?}, Dark {dark:?}, viewport {view:?})"
    );

    focus_search(&app, &mut vcx);
    assert!(
        walk_to(&mut vcx, "theme-mode-Light", "tab", false),
        "FOCUS: reach the Light card"
    );
    let at = offset_y(&app, &mut vcx);
    assert_eq!(
        at,
        px(0.),
        "setup: reaching the first page stop scrolled nothing"
    );
    key_settled(&mut vcx, "tab");
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("theme-mode-Dark"),
        "exactly one Tab from Light is the Dark card (no stop between them)"
    );
    assert_eq!(
        offset_y(&app, &mut vcx),
        at,
        "Tab between two visible stops scrolled the page"
    );
}

/// v5: Shift-Tab back up to a control above the viewport scrolls up to it.
#[gpui::test]
fn v5_shift_tab_to_a_control_above_the_fold_scrolls_up_minimally(cx: &mut TestAppContext) {
    let (app, mut vcx) = narrow(cx, SettingsSection::Appearance, NARROW.1);
    focus_search(&app, &mut vcx);
    assert!(
        walk_to(&mut vcx, TARGET, "tab", false),
        "FOCUS: reach {TARGET}"
    );
    let down = offset_y(&app, &mut vcx);
    assert!(down < px(0.), "setup: scrolled down");
    assert!(
        walk_to(&mut vcx, "font-size", "shift-tab", false),
        "FOCUS: Shift-Tab never came back to font-size"
    );
    let view = viewport(&app, &mut vcx);
    let control = vcx.debug_bounds("font-size").expect("drawn");
    assert!(offset_y(&app, &mut vcx) > down, "the page scrolled back up");
    assert!(
        within(control, view),
        "VISIBLE: {control:?} inside {view:?}"
    );
}

/// v6: a pointer click on a half-clipped control focuses it and scrolls nothing.
#[gpui::test]
fn v6_a_click_on_a_partly_clipped_control_does_not_scroll_the_page(cx: &mut TestAppContext) {
    // A short window, so the page has plenty of scroll range for the push.
    let (app, mut vcx) = narrow(cx, SettingsSection::Appearance, NARROW.1);
    let trigger = "cursor-style-trigger";
    let view = viewport(&app, &mut vcx);
    let b = vcx.debug_bounds(trigger).expect("drawn");
    // Push the page up until the control's top is 6px above the viewport.
    let push = (b.top() - view.top()) + px(6.);
    let max = state(&app, &mut vcx, |s| s.content_scroll.max_offset().y);
    assert!(
        push <= max,
        "setup: the push {push:?} that puts {trigger} 6px above the viewport top must fit \
         the page's scroll range {max:?} (control {b:?}, viewport {view:?}); gpui clamps \
         the offset to that range"
    );
    app.update_in(&mut vcx, |app, _, _| {
        let s = app.active_settings().unwrap();
        let o = s.content_scroll.offset();
        s.content_scroll.set_offset(point(o.x, o.y - push));
    });
    settle(&mut vcx);
    let clipped = vcx.debug_bounds(trigger).expect("drawn");
    assert!(
        (clipped.top() - (view.top() - px(6.))).abs() <= px(1.),
        "setup: the control's top is 6px above the viewport top (control {clipped:?}, \
         viewport {view:?})"
    );
    assert!(
        clipped.bottom() - view.top() >= px(8.),
        "setup: at least 8px of the control are still visible (control {clipped:?}, \
         viewport {view:?})"
    );
    let before = offset_y(&app, &mut vcx);
    let at = point(
        clipped.center().x,
        view.top() + (clipped.bottom() - view.top()) / 2.0,
    );
    vcx.simulate_click(at, Modifiers::none());
    settle(&mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(trigger),
        "FOCUS: the click focused the trigger"
    );
    assert_eq!(
        offset_y(&app, &mut vcx),
        before,
        "a pointer click moved the page under the pointer"
    );
}

/// v7: stops that never opt in (the nav rail's items and the modified-only row)
/// never scroll the page, however far it is scrolled.
#[gpui::test]
fn v7_pinned_stops_never_scroll_the_page(cx: &mut TestAppContext) {
    let (app, mut vcx) = narrow(cx, SettingsSection::Appearance, NARROW.1);
    app.update_in(&mut vcx, |app, _, _| {
        let s = app.active_settings().unwrap();
        s.content_scroll.set_offset(point(px(0.), px(-120.)));
    });
    settle(&mut vcx);
    let at = offset_y(&app, &mut vcx);
    assert!(at < px(0.), "setup: the page is scrolled");
    focus_search(&app, &mut vcx);
    let stop = "settings-modified-filter";
    assert!(walk_to(&mut vcx, stop, "tab", false), "FOCUS: reach {stop}");
    assert_eq!(offset_y(&app, &mut vcx), at, "{stop} scrolled the page");
    // A nav item, reached by Shift-Tab from the modified row's neighbours.
    let nav = walk_nav(&mut vcx);
    assert!(nav.is_some(), "FOCUS: a nav stop was never reached");
    assert_eq!(offset_y(&app, &mut vcx), at, "a nav stop scrolled the page");
}

fn walk_nav(vcx: &mut VisualTestContext) -> Option<String> {
    let mut seen = vec![focused(vcx)];
    for _ in 0..MAX_TABS {
        if let Some(m) = marker(vcx).filter(|m| m.starts_with("settings-nav-")) {
            return Some(m);
        }
        key_settled(vcx, "shift-tab");
        let now = focused(vcx);
        if now.is_none() || seen.contains(&now) {
            return marker(vcx).filter(|m| m.starts_with("settings-nav-"));
        }
        seen.push(now);
    }
    None
}

/// v8: on the search-results page every page control a real Tab reaches is
/// brought on screen.
#[gpui::test]
fn v8_every_stop_reached_on_the_results_page_is_visible(cx: &mut TestAppContext) {
    let (app, mut vcx) = narrow(cx, SettingsSection::General, 300.);
    app.update_in(&mut vcx, |app, window, cx| {
        let search = app.active_settings().unwrap().search.clone();
        search.update(cx, |i, cx| i.set_value("mouse".to_string(), window, cx));
        app.autoselect_settings_search(cx);
    });
    settle(&mut vcx);
    focus_search(&app, &mut vcx);
    let view = viewport(&app, &mut vcx);
    let mut checked = 0;
    let mut seen = vec![focused(&mut vcx)];
    for _ in 0..MAX_TABS {
        key_settled(&mut vcx, "tab");
        let now = focused(&mut vcx);
        if now.is_none() || seen.contains(&now) {
            break;
        }
        seen.push(now);
        let Some(id) = marker(&mut vcx).filter(|m| !m.starts_with("settings-")) else {
            continue;
        };
        if let Some(control) = vcx.debug_bounds(leak(id.clone())) {
            assert!(
                within(control, view),
                "{id} {control:?} was reached but is not inside the viewport {view:?}"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 2,
        "setup: the results page has stops to check ({checked})"
    );
}

/// v9: in the bounded host list (an inner scroller) every host head a real Tab
/// reaches is inside BOTH the list's viewport and the page's.
#[gpui::test]
fn v9_host_heads_in_the_inner_list_are_revealed_inner_then_outer(cx: &mut TestAppContext) {
    let (app, mut vcx) = raw_harness(cx);
    vcx.update(|w, _| w.activate_window());
    settle(&mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        cx.global_mut::<Config>().ssh_profiles = (0..40)
            .map(|i| {
                let mut p = crate::core::ssh_profile::SshProfile::new(format!("host-{i:02}"));
                p.host = format!("h{i}.example");
                p
            })
            .collect();
        app.open_settings_section(SettingsSection::Ssh, window, cx);
        app.active_settings_mut().unwrap().ssh_show_all = true;
    });
    vcx.simulate_resize(size(px(NARROW.0), px(520.)));
    settle(&mut vcx);
    focus_search(&app, &mut vcx);
    let initial_list = state(&app, &mut vcx, |s| s.ssh_list_scroll.bounds());
    assert!(
        initial_list.size.height > px(0.),
        "setup: the host list is its own scroller"
    );
    let mut checked = 0;
    let mut seen = vec![focused(&mut vcx)];
    for _ in 0..MAX_TABS {
        key_settled(&mut vcx, "tab");
        let now = focused(&mut vcx);
        if now.is_none() || seen.contains(&now) {
            break;
        }
        seen.push(now);
        let Some(id) = marker(&mut vcx).filter(|m| m.starts_with("ssh-host-")) else {
            continue;
        };
        let head = vcx.debug_bounds(leak(id.clone())).expect("drawn");
        // The page's reveal can move the list under the window, so both
        // viewports are read NOW, after this stop's reveal, not cached.
        let page = viewport(&app, &mut vcx);
        let list = state(&app, &mut vcx, |s| s.ssh_list_scroll.bounds());
        assert!(
            within(head, page),
            "{id} {head:?} is outside the page viewport {page:?}"
        );
        assert!(
            within(head, list),
            "{id} {head:?} is outside the list viewport {list:?}"
        );
        checked += 1;
        if checked >= 12 {
            break;
        }
    }
    assert!(
        checked >= 5,
        "setup: enough host heads were reached ({checked})"
    );
}

/// o2: after settings is closed and opened again, the stops move the NEW
/// page's scroller; the old page's scroller is never touched.
#[gpui::test]
fn o2_a_reopened_page_scrolls_its_own_scroller_not_a_stale_one(cx: &mut TestAppContext) {
    let (app, mut vcx) = narrow(cx, SettingsSection::Appearance, NARROW.1);
    focus_search(&app, &mut vcx);
    assert!(
        walk_to(&mut vcx, TARGET, "tab", false),
        "FOCUS: reach {TARGET}"
    );
    let old = state(&app, &mut vcx, |s| s.content_scroll.clone());
    let old_offset = old.offset().y;
    assert!(old_offset < px(0.), "setup: the first page scrolled");

    // Close with a real Escape (nothing else to peel), open the same page again.
    press(&mut vcx, "escape");
    assert!(
        app.update_in(&mut vcx, |app, _, _| app.active_settings().is_none()),
        "setup: Escape closed settings"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Appearance, window, cx);
    });
    settle(&mut vcx);
    focus_search(&app, &mut vcx);
    assert!(
        walk_to(&mut vcx, TARGET, "tab", false),
        "FOCUS: reach {TARGET} again"
    );
    let view = viewport(&app, &mut vcx);
    let control = vcx.debug_bounds(TARGET).expect("drawn");
    assert!(
        within(control, view),
        "VISIBLE on the new page: {control:?} in {view:?}"
    );
    assert!(
        state(&app, &mut vcx, |s| s.content_scroll.offset().y) < px(0.),
        "the NEW page's scroller moved"
    );
    assert_eq!(
        old.offset().y,
        old_offset,
        "the closed page's scroller was moved by a stop of the new page"
    );
}

// ------------------------------------------------------------------ p1: a padded stop

struct Scrolling {
    scroll: gpui::ScrollHandle,
}

impl Render for Scrolling {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // A 150px viewport over a column: a spacer, then a real `kit::Stop`
        // around a positioned clickable div with VERTICAL padding (py 12, fixed
        // 60px high, flex content START-aligned: a centred flex container would
        // centre a full-height absolute child back onto the control's origin and
        // cancel the padding), so a measuring box left to the auto
        // position inside that padding would sit 12px off the control's origin
        // in y, then more room below.
        div().p(px(10.)).child(
            div()
                .id("scrolling")
                .h(px(150.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(
                    gpui_component::v_flex()
                        .child(div().h(px(400.)))
                        .child(
                            kit::stop(
                                "pad-btn",
                                px(6.),
                                div()
                                    .id("pad-btn")
                                    .relative()
                                    .h(px(60.))
                                    .py(px(12.))
                                    .px(px(12.))
                                    .flex()
                                    .items_start()
                                    .child("Padded")
                                    .on_click(|_, _, _| {})
                                    .debug_selector(|| "pad-btn".to_string()),
                            )
                            .in_scroll(Some(self.scroll.clone())),
                        )
                        .child(div().h(px(300.))),
                ),
        )
    }
}

/// p1 (post-implementation, honest about reach): a REAL key sets the keyboard
/// modality the reveal requires, then `focus_next` (programmatic) reaches the
/// padded button below the viewport. The reveal under test is the same; what is
/// asserted comes from the ACTUAL button's debug bounds and the scroller's own
/// viewport, never from the measuring canvas the reveal used: the button's
/// bottom plus the ring's reach meets the viewport's bottom.
#[gpui::test]
fn p1_a_padded_stop_is_revealed_by_its_own_bounds(cx: &mut TestAppContext) {
    let (_app, _main) = raw_harness(cx);
    let scroll = gpui::ScrollHandle::new();
    let held = scroll.clone();
    let window = cx.add_window(move |_, _| Scrolling { scroll: held });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(400.), px(300.)));
    vcx.update(|w, _| w.activate_window());
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| w.is_window_active()),
        "setup: the window is active"
    );
    let view = scroll.bounds();
    let before = vcx.debug_bounds("pad-btn").expect("laid out");
    assert!(
        before.top() > view.bottom(),
        "setup: the button starts below the viewport ({before:?} vs {view:?})"
    );

    // A real key (not bound to anything) makes the last input the keyboard.
    key_settled(&mut vcx, "a");
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("pad-btn"),
        "FOCUS: the padded button holds focus"
    );

    let control = vcx.debug_bounds("pad-btn").expect("drawn");
    let outset = gap(&mut vcx) + crate::ui::theme::COMMIT_RING_W;
    let slack = scroll.bounds().bottom() - (control.bottom() + outset);
    assert!(
        scroll.offset().y < px(0.),
        "the scroller moved ({:?})",
        scroll.offset()
    );
    assert!(
        slack.abs() <= px(0.5),
        "minimal reveal from the control's own bounds: its bottom {:?} plus the ring's reach \
         {outset:?} should meet the viewport bottom {:?} (slack {slack:?})",
        control.bottom(),
        scroll.bounds().bottom()
    );
}

// ------------------------------------------------------------------ t1: the popover footer

/// t1: the theme popover's footer links float over the page (a deferred
/// popover), so they never opt into the page's scroller. With the popover open
/// by real keys, a real Tab to a footer stop leaves the page where it is and the
/// popover open. The stop is only reached, never activated (no fork, no folder).
#[gpui::test]
fn t1_a_theme_popover_footer_stop_never_scrolls_the_page(cx: &mut TestAppContext) {
    let (app, mut vcx) = raw_harness(cx);
    vcx.update(|w, _| w.activate_window());
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| w.is_window_active()),
        "setup: the test window is active"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        // System mode: both slot dropdowns are enabled.
        cx.global_mut::<Config>().theme_follow_system = true;
        app.open_settings_section(SettingsSection::Appearance, window, cx);
    });
    vcx.simulate_resize(size(px(NARROW.0), px(NARROW.1)));
    settle(&mut vcx);

    focus_search(&app, &mut vcx);
    assert!(
        walk_to(&mut vcx, "theme-slot-light-trigger", "tab", false),
        "FOCUS: Tab never reached the light-theme dropdown"
    );
    press(&mut vcx, "enter");
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu.clone()).as_deref(),
        Some("theme-slot-light"),
        "setup: Enter opened the theme popover"
    );
    let at = offset_y(&app, &mut vcx);

    assert!(
        walk_to(&mut vcx, "theme-duplicate", "tab", false),
        "FOCUS: Tab never reached the popover's footer stop"
    );
    assert_eq!(
        offset_y(&app, &mut vcx),
        at,
        "a footer stop of the floating popover scrolled the page"
    );
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu.clone()).as_deref(),
        Some("theme-slot-light"),
        "reaching it did not close the popover (and nothing was activated)"
    );
}

// ------------------------------------------------------------------ f1: the Files switch

/// f1: NEW with the rebase onto the current main, whose General page carries a
/// Files control (`wt-files-show-hidden`) that the 785 foundation does not have.
/// It is NOT run on 785 and its setup failure there is not a RED: it is for the
/// merged tree's GREEN.
///
/// The control is proved to start below the fold from its OWN bounds, with the
/// page at its top, BEFORE any focus moves. Then only real Tabs and
/// `run_until_parked` (no explicit draw) reach it, and what is asserted comes
/// from the actual control's bounds and the live viewport: the page scrolled
/// down, the control is inside the viewport, and so is the control plus the
/// focus ring's reach (the live ring offset and the ring's width).
#[gpui::test]
fn f1_a_real_tab_reveals_the_files_show_hidden_switch_on_narrow_general(cx: &mut TestAppContext) {
    const FILES_SWITCH: &str = "wt-files-show-hidden";
    let (app, mut vcx) = narrow(cx, SettingsSection::General, NARROW.1);
    let view = viewport(&app, &mut vcx);
    assert!(
        view.size.height > px(0.),
        "setup: the page is laid out ({view:?})"
    );
    assert_eq!(
        offset_y(&app, &mut vcx),
        px(0.),
        "setup: the page is at its top before any focus moves"
    );
    let before = vcx
        .debug_bounds(FILES_SWITCH)
        .expect("setup: the Files switch is laid out");
    assert!(
        before.top() > view.bottom(),
        "setup: starts below the viewport ({before:?} vs {view:?})"
    );
    assert_ne!(
        marker(&mut vcx).as_deref(),
        Some(FILES_SWITCH),
        "setup: the switch does not already hold focus"
    );

    focus_search(&app, &mut vcx);
    assert!(
        walk_to(&mut vcx, FILES_SWITCH, "tab", true),
        "FOCUS: Tab never reached {FILES_SWITCH}"
    );

    let view = viewport(&app, &mut vcx);
    let control = vcx
        .debug_bounds(FILES_SWITCH)
        .expect("still laid out after the reveal");
    assert!(
        offset_y(&app, &mut vcx) < px(0.),
        "the page scrolled down to the focused control"
    );
    assert!(
        within(control, view),
        "VISIBLE with no explicit draw: {control:?} inside {view:?}"
    );
    let outset = gap(&mut vcx) + crate::ui::theme::COMMIT_RING_W;
    let slack = px(0.5);
    assert!(
        control.top() - outset >= view.top() - slack
            && control.bottom() + outset <= view.bottom() + slack,
        "the control {control:?} plus the focus ring's reach ({outset:?}) is not inside the \
         viewport {view:?}"
    );
}
