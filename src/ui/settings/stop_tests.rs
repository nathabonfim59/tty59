//! Stage 1 reviewer controls for `kit::Stop` and the keyed Btn / Switch stops,
//! written against the real kit controls (not a copy of them):
//! * geometry: the same builder rendered bare and through the stop, in six
//!   parent classes, lays out every control and sibling identically;
//! * lifecycle: a stop's handle lives exactly as long as the control is drawn,
//!   a re-added control gets a fresh one, two same-id controls under different
//!   parents get one each, and controls with nothing to do are not stops;
//! * ring: the keyboard ring is the control grown by the installed gap and the
//!   shared stroke, is absent for pointer focus and when focus leaves, and wraps
//!   the control (not a stretched row) where the parent stretches.
//! Post-implementation controls: they cannot run before `Stop` exists.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::kit::test_support::harness;
use super::*;
use gpui::{
    Bounds, Context, KeyContext, Modifiers, Pixels, Render, TestAppContext, VisualTestContext,
    Window, div, px, size,
};
use gpui_component::{h_flex, v_flex};

fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..3 {
        vcx.run_until_parked();
        vcx.update(|w, cx| {
            let _ = w.draw(cx);
        });
    }
}

fn marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|w, _| {
        w.context_stack()
            .iter()
            .rev()
            .find_map(|c: &KeyContext| c.get(kit::STOP_MARK).map(|v| v.to_string()))
    })
}

fn focused(vcx: &mut VisualTestContext) -> Option<gpui::FocusHandle> {
    vcx.update(|w, cx| w.focused(cx))
}

fn grown(b: Bounds<Pixels>, by: Pixels) -> Bounds<Pixels> {
    Bounds {
        origin: gpui::point(b.origin.x - by, b.origin.y - by),
        size: size(b.size.width + by * 2.0, b.size.height + by * 2.0),
    }
}

fn near(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    let close = |x: Pixels, y: Pixels| (x - y).abs() <= px(0.5);
    close(a.origin.x, b.origin.x)
        && close(a.origin.y, b.origin.y)
        && close(a.size.width, b.size.width)
        && close(a.size.height, b.size.height)
}

fn ring_reach(vcx: &mut VisualTestContext) -> Pixels {
    vcx.update(|_, cx| {
        use gpui_component::ActiveTheme as _;
        cx.theme().focus_ring_offset()
    }) + crate::ui::theme::COMMIT_RING_W
}

// ------------------------------------------------------------------ geometry

#[derive(Clone, Copy, Debug)]
enum Parent {
    /// `h_flex` items_center, a flex_1 filter beside the control (hosts toolbar).
    Toolbar,
    /// A plain row (align-items stretch) with a taller sibling.
    StretchRow,
    /// A `v_flex` child that fills the width (nav item, host head): ad-hoc div.
    FullWidthRow,
    /// A stacked settings-row cell: `h_flex().w_full().justify_end()`.
    StackedCell,
    /// A `relative` parent that hangs an absolute popover off the control.
    PopoverAnchor,
    /// A control placed by an absolute wrapper (the close tile).
    AbsoluteTile,
    /// A `v_flex().items_start()` column, as the page's lone links sit.
    ColumnStart,
}

const ALL_PARENTS: [Parent; 7] = [
    Parent::Toolbar,
    Parent::StretchRow,
    Parent::FullWidthRow,
    Parent::StackedCell,
    Parent::PopoverAnchor,
    Parent::AbsoluteTile,
    Parent::ColumnStart,
];

/// Every kind of kit control that becomes a stop through its own render.
#[derive(Clone, Copy, Debug)]
enum Ctl {
    Secondary,
    Primary,
    Link,
    Danger,
    Switch,
    SwitchSmall,
}

const ALL_CTLS: [Ctl; 6] = [
    Ctl::Secondary,
    Ctl::Primary,
    Ctl::Link,
    Ctl::Danger,
    Ctl::Switch,
    Ctl::SwitchSmall,
];

const SELECTORS: [&str; 5] = ["ctl", "sib-a", "sib-b", "popover", "anchor"];

struct Geo {
    parent: Parent,
    ctl: Ctl,
    stopped: bool,
    anchor: Option<gpui::FocusHandle>,
}

fn sib(name: &'static str) -> gpui::Stateful<gpui::Div> {
    div().id(name).debug_selector(|| name.to_string())
}

/// The control under test. A Btn is bare without a handler and a stop with
/// one: the same builder, one switch. The ad-hoc div is bare, or the same div
/// through `kit::stop`.
fn ctl_el(ctl: Ctl, stopped: bool) -> gpui::AnyElement {
    let kind = match ctl {
        Ctl::Secondary => kit::BtnKind::Secondary,
        Ctl::Primary => kit::BtnKind::Primary,
        Ctl::Link => kit::BtnKind::Link,
        Ctl::Danger => kit::BtnKind::Danger,
        Ctl::Switch | Ctl::SwitchSmall => {
            let s = kit::switch("ctl").checked(true);
            let s = if matches!(ctl, Ctl::SwitchSmall) {
                s.small()
            } else {
                s
            };
            return if stopped {
                s.on_click(|_, _, _| {}).into_any_element()
            } else {
                s.into_any_element()
            };
        }
    };
    let b = kit::button("ctl", "Label", kind);
    if stopped {
        b.on_click(|_, _, _| {}).into_any_element()
    } else {
        b.into_any_element()
    }
}

/// An ad-hoc control div. Positioned (`relative`) bare and stopped alike: the
/// stop's ring layer needs a positioned control, and position alone moves
/// nothing.
fn tile_div() -> gpui::Stateful<gpui::Div> {
    div()
        .id("ctl")
        .relative()
        .h(px(30.))
        .flex()
        .items_center()
        .px(px(10.))
        .child("Row")
        .debug_selector(|| "ctl".to_string())
}

impl Render for Geo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let stopped = self.stopped;
        let body = match self.parent {
            Parent::Toolbar => h_flex()
                .w(px(500.))
                .gap(px(12.))
                .child(sib("sib-a").flex_1().min_w(px(100.)).h(px(26.)))
                .child(ctl_el(self.ctl, stopped))
                .child(sib("sib-b").w(px(40.)).h(px(20.)))
                .into_any_element(),
            Parent::StretchRow => div()
                .flex()
                .flex_row()
                .gap(px(8.))
                .child(ctl_el(self.ctl, stopped))
                .child(sib("sib-a").w(px(50.)).h(px(60.)))
                .into_any_element(),
            Parent::FullWidthRow => v_flex()
                .w(px(400.))
                .child(if stopped {
                    kit::stop("ctl", px(6.), tile_div()).into_any_element()
                } else {
                    tile_div().into_any_element()
                })
                .child(sib("sib-a").h(px(20.)))
                .into_any_element(),
            Parent::StackedCell => h_flex()
                .w(px(300.))
                .justify_end()
                .gap(px(8.))
                .child(sib("sib-a").w(px(60.)).h(px(20.)))
                .child(ctl_el(self.ctl, stopped))
                .into_any_element(),
            Parent::PopoverAnchor => div()
                .relative()
                .flex_shrink_0()
                .child(ctl_el(self.ctl, stopped))
                .child(
                    sib("popover")
                        .absolute()
                        .top(px(34.))
                        .right_0()
                        .w(px(90.))
                        .h(px(40.)),
                )
                .into_any_element(),
            Parent::AbsoluteTile => sib("anchor")
                .relative()
                .w(px(300.))
                .h(px(120.))
                .child(
                    div()
                        .absolute()
                        .top(px(10.))
                        .right(px(12.))
                        .child(if stopped {
                            kit::stop("ctl", px(6.), tile_div().w(px(28.))).into_any_element()
                        } else {
                            tile_div().w(px(28.)).into_any_element()
                        }),
                )
                // A sibling in flow under the positioned tile, so the container's
                // own box and what lies beside the tile are compared too.
                .child(sib("sib-a").mt(px(80.)).w(px(50.)).h(px(20.)))
                .into_any_element(),
            Parent::ColumnStart => v_flex()
                .items_start()
                .w(px(300.))
                .gap(px(8.))
                .child(ctl_el(self.ctl, stopped))
                .child(sib("sib-a").w(px(60.)).h(px(20.)))
                .into_any_element(),
        };
        // Same root for every case. The ring tests give it a focus anchor (a
        // non-stop handle) to start a real Tab from; see `ring_view`.
        let root = div().id("geo-root");
        let root = match &self.anchor {
            Some(anchor) => root.track_focus(anchor),
            None => root,
        };
        root.p(px(30.)).child(body)
    }
}

fn geometry(
    cx: &mut TestAppContext,
    parent: Parent,
    ctl: Ctl,
    stopped: bool,
) -> Vec<(&'static str, Bounds<Pixels>)> {
    let window = cx.add_window(move |_, _| Geo {
        parent,
        ctl,
        stopped,
        anchor: None,
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(600.), px(260.)));
    settle(&mut vcx);
    SELECTORS
        .iter()
        .filter_map(|s| vcx.debug_bounds(s).map(|b| (*s, b)))
        .collect()
}

/// The same builder bare and through the stop, in `parent`: every control and
/// sibling rect identical. `kit` controls are run as each of `ctls`; the
/// ad-hoc parents (a row div, the close tile) take no kit control and run once.
fn identical_for(cx: &mut TestAppContext, parent: Parent, ctls: &[Ctl]) {
    let (_app, _vcx) = harness(cx);
    for ctl in ctls {
        let bare = geometry(cx, parent, *ctl, false);
        let stopped = geometry(cx, parent, *ctl, true);
        assert!(
            bare.iter().any(|(s, _)| *s == "ctl") && bare.len() >= 2,
            "{parent:?} / {ctl:?}: setup, the control and a sibling or anchor are laid out ({bare:?})"
        );
        assert_eq!(
            stopped, bare,
            "{parent:?} / {ctl:?}: through the stop every control and sibling lies exactly where it did bare"
        );
    }
}

fn identical(cx: &mut TestAppContext, parent: Parent) {
    identical_for(cx, parent, &ALL_CTLS);
}

fn identical_adhoc(cx: &mut TestAppContext, parent: Parent) {
    identical_for(cx, parent, &[Ctl::Secondary]);
}

#[gpui::test]
fn g1_toolbar_with_a_flex_filter_is_unchanged(cx: &mut TestAppContext) {
    identical(cx, Parent::Toolbar);
}

#[gpui::test]
fn g2_stretch_row_with_a_taller_sibling_is_unchanged(cx: &mut TestAppContext) {
    identical(cx, Parent::StretchRow);
}

#[gpui::test]
fn g3_full_width_row_is_unchanged(cx: &mut TestAppContext) {
    identical_adhoc(cx, Parent::FullWidthRow);
}

#[gpui::test]
fn g4_stacked_cell_is_unchanged(cx: &mut TestAppContext) {
    identical(cx, Parent::StackedCell);
}

#[gpui::test]
fn g5_popover_anchor_is_unchanged(cx: &mut TestAppContext) {
    identical(cx, Parent::PopoverAnchor);
}

#[gpui::test]
fn g6_absolute_tile_is_unchanged(cx: &mut TestAppContext) {
    identical_adhoc(cx, Parent::AbsoluteTile);
}

#[gpui::test]
fn g7_column_start_is_unchanged(cx: &mut TestAppContext) {
    identical(cx, Parent::ColumnStart);
}

#[test]
fn g0_every_parent_class_is_covered() {
    assert_eq!(ALL_PARENTS.len(), 7);
}

// ------------------------------------------------------------------ lifecycle

struct Life {
    show: Rc<Cell<bool>>,
    kind: LifeKind,
}

#[derive(Clone, Copy)]
enum LifeKind {
    /// One interactive Btn "life" while `show`.
    One,
    /// Nothing to do: disabled Btn, handler-less Btn, handler-less Switch,
    /// disabled Switch with a handler.
    Inert,
    /// Two interactive Btns with the same id under two parents.
    Twins,
}

impl Render for Life {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let noop = |_: &gpui::ClickEvent, _: &mut Window, _: &mut gpui::App| {};
        let body =
            match self.kind {
                LifeKind::One => div()
                    .when(self.show.get(), |d| {
                        d.child(kit::button("life", "Go", kit::BtnKind::Secondary).on_click(noop))
                    })
                    .into_any_element(),
                LifeKind::Inert => v_flex()
                    .gap(px(8.))
                    .child(
                        kit::button("inert-disabled", "A", kit::BtnKind::Secondary)
                            .disabled(true)
                            .on_click(noop),
                    )
                    .child(kit::button("inert-none", "B", kit::BtnKind::Secondary))
                    .child(kit::switch("inert-switch").checked(true))
                    .child(
                        kit::switch("inert-switch-off")
                            .disabled(true)
                            .on_click(|_, _, _| {}),
                    )
                    .into_any_element(),
                LifeKind::Twins => {
                    v_flex()
                        .gap(px(8.))
                        // The parents carry explicit, distinct ids: gpui scopes keyed
                        // state by the element-id path, and an anonymous div pushes
                        // nothing onto it, so anonymous parents would NOT tell two
                        // same-id controls apart (and nor should this fixture claim
                        // they do).
                        .child(div().id("twin-a").child(
                            kit::button("dup", "One", kit::BtnKind::Secondary).on_click(noop),
                        ))
                        .child(div().id("twin-b").child(
                            kit::button("dup", "Two", kit::BtnKind::Secondary).on_click(noop),
                        ))
                        .into_any_element()
                }
            };
        div().p(px(30.)).child(body)
    }
}

fn life(
    cx: &mut TestAppContext,
    kind: LifeKind,
    show: bool,
) -> (VisualTestContext, Rc<Cell<bool>>) {
    let show = Rc::new(Cell::new(show));
    let flag = show.clone();
    let window = cx.add_window(move |_, _| Life { show: flag, kind });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(400.), px(300.)));
    settle(&mut vcx);
    (vcx, show)
}

#[gpui::test]
fn l1_a_control_no_longer_drawn_takes_its_handle_and_a_new_one_is_fresh(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let (mut vcx, show) = life(cx, LifeKind::One, true);
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("life"),
        "setup: Tab reaches the stop"
    );
    let first = focused(&mut vcx).expect("a handle").downgrade();

    show.set(false);
    vcx.update(|w, _| w.refresh());
    settle(&mut vcx);
    assert!(
        first.upgrade().is_none(),
        "the handle is gone with the control, not kept in a registry"
    );
    assert_eq!(marker(&mut vcx), None, "and so is the focus on it");

    show.set(true);
    vcx.update(|w, _| w.refresh());
    settle(&mut vcx);
    assert!(
        first.upgrade().is_none(),
        "the re-added control did not revive the old handle"
    );
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("life"),
        "the new one is a stop again"
    );
}

#[gpui::test]
fn l2_controls_with_nothing_to_do_are_not_stops(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let (mut inert, _) = life(cx, LifeKind::Inert, true);
    for _ in 0..4 {
        inert.update(|w, cx| w.focus_next(cx));
        settle(&mut inert);
        assert_eq!(marker(&mut inert), None, "an inert control took the focus");
        assert!(
            focused(&mut inert).is_none(),
            "an inert control is a tab stop"
        );
    }
    let (mut live, _) = life(cx, LifeKind::One, true);
    live.update(|w, cx| w.focus_next(cx));
    settle(&mut live);
    assert_eq!(
        marker(&mut live).as_deref(),
        Some("life"),
        "control: the same walk does find a real stop"
    );
}

#[gpui::test]
fn l3_same_id_under_different_parents_gets_a_handle_each(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let (mut vcx, _) = life(cx, LifeKind::Twins, true);
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    let first = focused(&mut vcx).expect("a first stop");
    assert_eq!(marker(&mut vcx).as_deref(), Some("dup"));
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    let second = focused(&mut vcx).expect("a second stop");
    assert_eq!(marker(&mut vcx).as_deref(), Some("dup"));
    assert!(first != second, "two controls with one id share a handle");
}

// ------------------------------------------------------------------ ring

/// The fixture is mounted under the actual `gpui_component::Root`, as
/// `harness()` mounts the app, and returns a focus anchor: a plain, non-stop
/// handle on the fixture's root div.
///
/// Why an anchor: Root binds `tab` to its own `Tab` action under the `Root` key
/// context (gpui-component root.rs:26), and gpui dispatches a key from the
/// FOCUSED element's node, falling back to the window's root node when nothing
/// is focused (gpui window.rs `focus_node_id_in_rendered_frame`). Root's element
/// is a child of that root node, so with nothing focused its context is not on
/// the path and Tab does nothing; in the app something always holds focus. So
/// the tests focus the anchor programmatically (setup, not a claim about
/// reach), then press a REAL Tab: Root's own handler runs `focus_next`, which
/// moves from the anchor to the first stop after it in tab order. No navigation
/// is copied here.
fn ring_view(cx: &mut TestAppContext, parent: Parent) -> (VisualTestContext, gpui::FocusHandle) {
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let window = cx.add_window(move |window, cx| {
        let view = cx.new(|cx| {
            let anchor = cx.focus_handle();
            *out.borrow_mut() = Some(anchor.clone());
            Geo {
                parent,
                ctl: Ctl::Secondary,
                stopped: true,
                anchor: Some(anchor),
            }
        });
        gpui_component::Root::new(view, window, cx)
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(600.), px(260.)));
    settle(&mut vcx);
    let anchor = slot.borrow().clone().expect("the fixture built its anchor");
    (vcx, anchor)
}

/// Setup: focus is on the non-stop anchor, no stop holds it yet.
fn focus_the_anchor(vcx: &mut VisualTestContext, anchor: &gpui::FocusHandle) {
    vcx.update(|w, cx| w.focus(anchor, cx));
    settle(vcx);
    assert!(
        vcx.update(|w, _| anchor.is_focused(w)),
        "setup: the anchor holds focus"
    );
    assert_eq!(marker(vcx), None, "setup: no stop is focused yet");
}

#[gpui::test]
fn k1_a_keyboard_focused_stop_draws_the_ring_around_its_control(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let (mut vcx, anchor) = ring_view(cx, Parent::Toolbar);
    assert!(
        vcx.debug_bounds("ctl-ring").is_none(),
        "no ring before focus"
    );
    focus_the_anchor(&mut vcx, &anchor);
    vcx.simulate_keystrokes("tab");
    settle(&mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("ctl"),
        "FOCUS: a real Tab, through Root's own handler, moved focus from the anchor to the stop"
    );
    let control = vcx.debug_bounds("ctl").expect("the control");
    let ring = vcx
        .debug_bounds("ctl-ring")
        .expect("PAINT: a keyboard-focused stop draws a ring");
    let by = ring_reach(&mut vcx);
    assert!(
        near(ring, grown(control, by)),
        "the ring {ring:?} is the control {control:?} grown by the gap and the stroke ({by:?})"
    );
}

#[gpui::test]
fn k2_a_pointer_click_focuses_the_stop_but_draws_no_ring(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let (mut vcx, _anchor) = ring_view(cx, Parent::Toolbar);
    let at = vcx.debug_bounds("ctl").expect("the control").center();
    vcx.simulate_click(at, Modifiers::none());
    settle(&mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("ctl"),
        "FOCUS: the click focused it"
    );
    assert!(
        vcx.debug_bounds("ctl-ring").is_none(),
        "PAINT: pointer-modality focus draws no ring"
    );
}

#[gpui::test]
fn k3_the_ring_goes_when_focus_leaves(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let (mut vcx, anchor) = ring_view(cx, Parent::Toolbar);
    focus_the_anchor(&mut vcx, &anchor);
    vcx.simulate_keystrokes("tab");
    settle(&mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("ctl"),
        "setup: a real Tab put focus on the stop"
    );
    assert!(
        vcx.debug_bounds("ctl-ring").is_some(),
        "setup: ringed while focused"
    );
    vcx.update(|w, _| w.blur());
    settle(&mut vcx);
    assert_eq!(marker(&mut vcx), None, "FOCUS: gone");
    assert!(
        vcx.debug_bounds("ctl-ring").is_none(),
        "PAINT: and so is the ring"
    );
}

#[gpui::test]
fn k4_in_a_stretching_row_the_ring_wraps_the_control_not_the_row(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let (mut vcx, anchor) = ring_view(cx, Parent::StretchRow);
    focus_the_anchor(&mut vcx, &anchor);
    vcx.simulate_keystrokes("tab");
    settle(&mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("ctl"),
        "FOCUS: a real Tab moved focus from the anchor to the stop"
    );
    let control = vcx.debug_bounds("ctl").expect("the control");
    let ring = vcx.debug_bounds("ctl-ring").expect("a ring");
    let by = ring_reach(&mut vcx);
    assert!(
        near(ring, grown(control, by)),
        "the ring {ring:?} is the 26px control {control:?} grown, not the 60px row"
    );
}

// ------------------------------------------------------------------ delivered keys

struct Keyed {
    log: Rc<RefCell<Vec<String>>>,
    clicks: Rc<Cell<usize>>,
}

impl Render for Keyed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let log = self.log.clone();
        let clicks = self.clicks.clone();
        div().p(px(30.)).child(
            kit::stop(
                "keyed",
                px(6.),
                div()
                    .id("keyed")
                    .w(px(80.))
                    .h(px(26.))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
            .on_key(move |ev, _, _| log.borrow_mut().push(ev.keystroke.key.to_string())),
        )
    }
}

/// `on_key` hands the stop's own callback the key as the keyboard delivered
/// it, while the control is focused and only then; Enter and Space still reach
/// the control's click. The one control that proves the builder before any
/// caller (the stepper, the slider, the dropdown trigger) relies on it.
#[gpui::test]
fn k5_a_focused_stop_is_handed_the_keys_and_enter_still_clicks(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let clicks = Rc::new(Cell::new(0usize));
    let (l, c) = (log.clone(), clicks.clone());
    let window = cx.add_window(move |_, _| Keyed { log: l, clicks: c });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(300.), px(160.)));
    settle(&mut vcx);

    let press = |vcx: &mut VisualTestContext, key: &str| {
        vcx.simulate_keystrokes(key);
        vcx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse(key).expect("a key name"),
        });
        settle(vcx);
    };

    press(&mut vcx, "right");
    assert!(
        log.borrow().is_empty(),
        "setup: nothing is focused yet, so no key is delivered ({:?})",
        log.borrow()
    );
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("keyed"),
        "FOCUS: the stop holds focus"
    );

    press(&mut vcx, "right");
    assert_eq!(
        *log.borrow(),
        vec!["right".to_string()],
        "the arrow is delivered once"
    );
    assert_eq!(clicks.get(), 0, "an arrow is not a click");
    press(&mut vcx, "enter");
    assert_eq!(
        *log.borrow(),
        vec!["right".to_string(), "enter".to_string()],
        "Enter is delivered too"
    );
    assert_eq!(
        clicks.get(),
        1,
        "and its key-up is still the control's one click"
    );
    press(&mut vcx, "space");
    assert_eq!(clicks.get(), 2, "Space likewise");
}

// ------------------------------------------------------------------ callback counts (stepper, slider)
//
// The real `kit::stepper` and `kit::slider` with counting closures. A key press
// is one drag-and-release for the slider and one click for a stepper end, and
// a key the control does not answer to, or one at a limit, calls nothing.

struct Counted {
    dec: Rc<Cell<usize>>,
    inc: Rc<Cell<usize>>,
    can_inc: bool,
    sets: Rc<RefCell<Vec<f32>>>,
    releases: Rc<Cell<usize>>,
    fraction: f32,
    slider: bool,
    /// The keys that reached the control's parent: what the stop did not consume.
    parent_keys: Rc<RefCell<Vec<String>>>,
}

impl Render for Counted {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tk = kit::Tk::of(cx);
        let body = if self.slider {
            let sets = self.sets.clone();
            let releases = self.releases.clone();
            kit::slider(
                "cnt-slider",
                self.fraction,
                "readout".to_string(),
                &tk,
                Rc::new(move |f, _, _| sets.borrow_mut().push(f)),
                Rc::new(move |_, _| releases.set(releases.get() + 1)),
                None,
            )
        } else {
            let (dec, inc) = (self.dec.clone(), self.inc.clone());
            kit::stepper(
                "cnt-stepper",
                kit::Stepper {
                    value: "5".to_string(),
                    unit: " pt".to_string(),
                    can_dec: true,
                    can_inc: self.can_inc,
                },
                &tk,
                move |_, _, _| dec.set(dec.get() + 1),
                move |_, _, _| inc.set(inc.get() + 1),
                None,
            )
        };
        let parent_keys = self.parent_keys.clone();
        div()
            .p(px(30.))
            .on_key_down(move |ev, _, _| parent_keys.borrow_mut().push(ev.keystroke.unparse()))
            .child(body)
    }
}

struct CountedHandles {
    vcx: VisualTestContext,
    dec: Rc<Cell<usize>>,
    inc: Rc<Cell<usize>>,
    sets: Rc<RefCell<Vec<f32>>>,
    releases: Rc<Cell<usize>>,
    parent_keys: Rc<RefCell<Vec<String>>>,
}

fn counted(cx: &mut TestAppContext, slider: bool, fraction: f32, can_inc: bool) -> CountedHandles {
    let (dec, inc) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let sets = Rc::new(RefCell::new(Vec::new()));
    let releases = Rc::new(Cell::new(0));
    let parent_keys = Rc::new(RefCell::new(Vec::new()));
    let view = Counted {
        dec: dec.clone(),
        inc: inc.clone(),
        can_inc,
        sets: sets.clone(),
        releases: releases.clone(),
        fraction,
        slider,
        parent_keys: parent_keys.clone(),
    };
    let window = cx.add_window(move |_, _| view);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(400.), px(160.)));
    settle(&mut vcx);
    // The control is the only stop: the walk reaches it (reach through a real
    // Tab is R8/R9's), then real keys.
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    CountedHandles {
        vcx,
        dec,
        inc,
        sets,
        releases,
        parent_keys,
    }
}

fn press_key(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    vcx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse(key).expect("a key name"),
    });
    settle(vcx);
}

#[gpui::test]
fn c1_stepper_keys_call_each_live_end_once_per_press(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let mut h = counted(cx, false, 0., true);
    assert_eq!(
        marker(&mut h.vcx).as_deref(),
        Some("cnt-stepper"),
        "one stop"
    );
    for (key, dec, inc) in [
        ("right", 0, 1),
        ("up", 0, 2),
        ("left", 1, 2),
        ("down", 2, 2),
        ("a", 2, 2),
        ("enter", 2, 2),
    ] {
        press_key(&mut h.vcx, key);
        assert_eq!((h.dec.get(), h.inc.get()), (dec, inc), "after {key}");
    }
}

#[gpui::test]
fn c2_a_disabled_stepper_end_calls_nothing(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let mut h = counted(cx, false, 0., false);
    press_key(&mut h.vcx, "right");
    press_key(&mut h.vcx, "up");
    assert_eq!(h.inc.get(), 0, "the increment end is disabled");
    press_key(&mut h.vcx, "left");
    assert_eq!(h.dec.get(), 1, "the decrement end still answers");
}

#[gpui::test]
fn c3_slider_keys_set_then_release_once_per_press(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let mut h = counted(cx, true, 0.5, true);
    assert_eq!(
        marker(&mut h.vcx).as_deref(),
        Some("cnt-slider"),
        "the track is the stop"
    );
    let sets = |h: &CountedHandles| h.sets.borrow().clone();
    press_key(&mut h.vcx, "right");
    assert_eq!(
        (sets(&h).len(), h.releases.get()),
        (1, 1),
        "one set and one release"
    );
    assert!((sets(&h)[0] - 0.55).abs() < 1e-4, "0.05 up: {:?}", sets(&h));
    press_key(&mut h.vcx, "left");
    assert_eq!((sets(&h).len(), h.releases.get()), (2, 2));
    assert!(
        (sets(&h)[1] - 0.45).abs() < 1e-4,
        "0.05 down: {:?}",
        sets(&h)
    );
    press_key(&mut h.vcx, "home");
    assert_eq!((sets(&h).len(), h.releases.get()), (3, 3));
    assert_eq!(sets(&h)[2], 0., "Home is the start");
    press_key(&mut h.vcx, "end");
    assert_eq!((sets(&h).len(), h.releases.get()), (4, 4));
    assert_eq!(sets(&h)[3], 1., "End is the end");
    press_key(&mut h.vcx, "up");
    press_key(&mut h.vcx, "a");
    assert_eq!(
        (sets(&h).len(), h.releases.get()),
        (4, 4),
        "a key the slider does not answer to calls nothing"
    );
}

#[gpui::test]
fn c4_a_slider_key_at_its_limit_calls_nothing(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let mut h = counted(cx, true, 1.0, true);
    press_key(&mut h.vcx, "right");
    press_key(&mut h.vcx, "end");
    assert_eq!(
        (h.sets.borrow().len(), h.releases.get()),
        (0, 0),
        "already at the end: nothing to set, nothing to persist"
    );
    press_key(&mut h.vcx, "left");
    assert_eq!((h.sets.borrow().len(), h.releases.get()), (1, 1));
}

// ------------------------------------------------------------------ modifiers
//
// The arrow and Home/End keys a stop answers to are answered only plain. A
// chord with Ctrl, Alt, Shift or Super is not the control's: it calls nothing
// and is NOT consumed, so it still reaches whatever binds it. A plain key it
// does answer to is consumed (the parent never sees it).

const CHORDS: [&str; 4] = ["ctrl-right", "alt-right", "shift-right", "cmd-right"];

fn parent_saw(h: &CountedHandles) -> Vec<String> {
    h.parent_keys.borrow().clone()
}

#[gpui::test]
fn m1_a_modified_chord_never_steps_a_stepper_and_still_reaches_the_parent(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let mut h = counted(cx, false, 0., true);
    h.parent_keys.borrow_mut().clear();
    for chord in CHORDS {
        press_key(&mut h.vcx, chord);
    }
    assert_eq!((h.dec.get(), h.inc.get()), (0, 0), "no chord stepped it");
    let seen = parent_saw(&h);
    for chord in CHORDS {
        // The parent logs the platform's own spelling (Super is `super-` on
        // Linux, where the test types `cmd-`): compare the parsed chord.
        let want = gpui::Keystroke::parse(chord).expect("a chord").unparse();
        assert!(
            seen.iter().any(|k| *k == want),
            "{chord} ({want}) was consumed by the stepper: the parent saw {seen:?}"
        );
    }
    h.parent_keys.borrow_mut().clear();
    press_key(&mut h.vcx, "right");
    assert_eq!(h.inc.get(), 1, "the plain key steps it");
    assert!(
        parent_saw(&h).iter().all(|k| k != "right"),
        "and the plain key it answers to is consumed"
    );
}

#[gpui::test]
fn m2_a_modified_chord_never_moves_a_slider_and_still_reaches_the_parent(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let mut h = counted(cx, true, 0.5, true);
    h.parent_keys.borrow_mut().clear();
    let mut chords = CHORDS.to_vec();
    chords.extend(["ctrl-home", "shift-end"]);
    for chord in &chords {
        press_key(&mut h.vcx, chord);
    }
    assert_eq!(
        (h.sets.borrow().len(), h.releases.get()),
        (0, 0),
        "no chord set or persisted anything"
    );
    let seen = parent_saw(&h);
    for chord in &chords {
        let want = gpui::Keystroke::parse(chord).expect("a chord").unparse();
        assert!(
            seen.iter().any(|k| *k == want),
            "{chord} ({want}) was consumed by the slider: the parent saw {seen:?}"
        );
    }
    press_key(&mut h.vcx, "right");
    assert_eq!(h.sets.borrow().len(), 1, "the plain key still moves it");
}
