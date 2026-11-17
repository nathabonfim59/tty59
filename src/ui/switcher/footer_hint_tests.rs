//! POST-IMPLEMENTATION controls for `footer_hint`, the one builder every
//! footer hint goes through, laid out by the real layout in a small themed
//! window of its own.
//!
//! `footer_plan::hint_w` is arithmetic over measured widths. These read what
//! layout actually gave the real keycap nodes the footer draws, so a
//! keycap-only hint is checked against the keycap constant itself (18px) and
//! not against a second copy of the formula, and a hint that has to keep its
//! words is checked not to wrap or shrink when the box around it is narrower
//! than it is.

use gpui::{
    Bounds, Context, IntoElement, ParentElement as _, Pixels, Render, Styled as _, TestAppContext,
    Window, px,
};
use gpui_component::{h_flex, v_flex};

use super::{HintKind, footer_hint, footer_plan};
use crate::ui::app::test_window::harness;
use crate::ui::dialog::KEYCAP;
use crate::ui::paint_probe;

/// A sentence long enough that any width under its own would wrap it.
const WORDS: &str = "a footer wording long enough to wrap in a narrow box";
const NARROW_BOX: f32 = 30.;
/// Sub-pixel layout rounding only.
const EPS: f32 = 0.01;

struct Hints;

impl Render for Hints {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let key = |s: &str| s.to_string();
        // One keycap per glyph, each glyph its own, so a keycap's selector
        // (`dialog-keycap-{glyph}`) names exactly one node in the window.
        let one = [key("↵")];
        let two = [key("↑"), key("↓")];
        let wide = [key("A"), key("B")];
        let narrow = [key("C"), key("D")];
        v_flex()
            .items_start()
            .gap(px(8.))
            .child(footer_hint(
                HintKind::Probe("one-cap"),
                &one,
                None,
                px(0.),
                cx,
            ))
            .child(footer_hint(
                HintKind::Probe("two-caps"),
                &two,
                None,
                px(0.),
                cx,
            ))
            .child(h_flex().child(footer_hint(
                HintKind::Probe("words-wide"),
                &wide,
                Some(WORDS.to_string()),
                px(0.),
                cx,
            )))
            .child(
                h_flex()
                    .w(px(NARROW_BOX))
                    .overflow_hidden()
                    .child(footer_hint(
                        HintKind::Probe("words-narrow"),
                        &narrow,
                        Some(WORDS.to_string()),
                        px(0.),
                        cx,
                    )),
            )
    }
}

fn width(b: &Bounds<Pixels>) -> f32 {
    b.size.width.as_f32()
}

fn height(b: &Bounds<Pixels>) -> f32 {
    b.size.height.as_f32()
}

#[gpui::test]
fn keys_only_hints_are_their_real_keycaps_and_gaps(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    // The themed app window installs the globals the keycap reads its ink
    // from; the hints are drawn in a window of their own beside it.
    let (_app, _themed) = harness(cx);
    let (view, vcx) = cx.add_window_view(|_, _| Hints);
    paint_probe::clear();
    view.update(vcx, |_, cx| cx.notify());
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    let bounds = |vcx: &mut gpui::VisualTestContext, selector: &'static str| {
        vcx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not laid out"))
    };

    // The keycap constant the footer's own caps are built on.
    assert_eq!(KEYCAP, 18., "setup: the keycap is 18px");

    // One cap: exactly one keycap, no words, no gap.
    let one = bounds(vcx, "switcher-hint:one-cap");
    let enter = bounds(vcx, "dialog-keycap-↵");
    assert!(
        (width(&enter) - 18.).abs() <= EPS,
        "the keycap is {enter:?}"
    );
    assert!(
        (width(&one) - 18.).abs() <= EPS && (height(&one) - 18.).abs() <= EPS,
        "a one-cap keycap-only hint is 18x18, got {one:?}"
    );

    // Two caps: 18 + the 6px hint gap + 18, and nothing after the last cap.
    let two = bounds(vcx, "switcher-hint:two-caps");
    let (up, down) = (
        bounds(vcx, "dialog-keycap-↑"),
        bounds(vcx, "dialog-keycap-↓"),
    );
    assert!(
        (width(&up) - 18.).abs() <= EPS && (width(&down) - 18.).abs() <= EPS,
        "the keycaps are {up:?} and {down:?}"
    );
    assert!(
        (down.left().as_f32() - up.right().as_f32() - footer_plan::HINT_GAP).abs() <= EPS,
        "the caps sit {} apart, not the hint gap {}",
        down.left().as_f32() - up.right().as_f32(),
        footer_plan::HINT_GAP
    );
    assert!(
        (width(&two) - 42.).abs() <= EPS && (height(&two) - 18.).abs() <= EPS,
        "a two-cap keycap-only hint is 42x18, got {two:?}"
    );
    assert!(
        (width(&two) - (2. * KEYCAP + footer_plan::HINT_GAP)).abs() <= EPS,
        "42 is two keycaps and one gap"
    );

    // A keycap-only hint is built with an empty label; one with words, with
    // them.
    assert_eq!(
        paint_probe::label("switcher-hint:one-cap").as_deref(),
        Some("")
    );
    assert_eq!(
        paint_probe::label("switcher-hint:two-caps").as_deref(),
        Some("")
    );
    assert_eq!(
        paint_probe::label("switcher-hint:words-wide").as_deref(),
        Some(WORDS)
    );
}

#[gpui::test]
fn a_hint_with_words_neither_shrinks_nor_wraps_in_a_box_too_small_for_it(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let (_app, _themed) = harness(cx);
    let (view, vcx) = cx.add_window_view(|_, _| Hints);
    paint_probe::clear();
    view.update(vcx, |_, cx| cx.notify());
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    let wide = vcx
        .debug_bounds("switcher-hint:words-wide")
        .expect("the hint with room was laid out");
    let narrow = vcx
        .debug_bounds("switcher-hint:words-narrow")
        .expect("the hint in the narrow box was laid out");
    assert!(
        width(&wide) > NARROW_BOX + 1.,
        "setup: the hint wants more than the {NARROW_BOX}px box has: {wide:?}"
    );
    assert!(
        (width(&narrow) - width(&wide)).abs() <= EPS
            && (height(&narrow) - height(&wide)).abs() <= EPS,
        "squeezed or wrapped: {narrow:?} is not its {wide:?} with room"
    );
}
