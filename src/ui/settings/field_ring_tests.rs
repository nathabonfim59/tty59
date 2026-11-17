//! The settings text fields' keyboard focus indication. Today a focused field
//! shows only its own 1px `k45` inset ring (or the danger ring when invalid); that
//! ring measured under 3:1 on some themes, so the field is to carry the shared
//! focus ring too, drawn INSIDE its box over the Input's own handle (no new tab
//! stop, click focus included). These tests are written against the real pages
//! before that change and say what must stay as well as what must appear:
//!
//! * the field box is found by its Input entity (`settings-field-{id}`), its
//!   measured bounds and its own FIRST box shadow are read from that one field
//!   (`paint_probe` key `settings.field-{id}`), never selected by an expected
//!   colour;
//! * the overlay is found as `settings-field-{id}-ring`;
//! * the error line of a checked input is found by the same entity
//!   (`settings-field-{id}-error`) and its label is the text the page drew.
//!
//! Tests that only describe what already works (box, shadow, error line, bounds)
//! pass today and must keep passing; the overlay assertions are the RED. Ink
//! contrast of the overlay against the page is the native measure, not here.

use super::kit::test_support::harness;
use super::*;
use crate::ui::paint_probe;
use gpui::{Modifiers, TestAppContext, VisualTestContext, size};
use gpui_component::input::InputState;

fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..3 {
        vcx.run_until_parked();
        vcx.update(|w, cx| {
            let _ = w.draw(cx);
        });
    }
}

/// Settle with a fresh set of probe notes, so what is read is this frame's.
fn settle_fresh(vcx: &mut VisualTestContext) {
    paint_probe::clear();
    settle(vcx);
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn id_of(input: &Entity<InputState>) -> u64 {
    input.entity_id().as_u64()
}

fn bounds_of(vcx: &mut VisualTestContext, id: u64) -> Option<gpui::Bounds<gpui::Pixels>> {
    vcx.debug_bounds(leak(format!("settings-field-{id}")))
}

fn ring_of(vcx: &mut VisualTestContext, id: u64) -> Option<gpui::Bounds<gpui::Pixels>> {
    vcx.debug_bounds(leak(format!("settings-field-{id}-ring")))
}

/// The field's own painted box, as the last frame noted it.
fn painted(id: u64) -> paint_probe::Painted {
    paint_probe::all(&format!("settings.field-{id}"))
        .last()
        .copied()
        .unwrap_or_else(|| panic!("the field {id} was not noted this frame"))
}

fn tk(vcx: &mut VisualTestContext) -> kit::Tk {
    vcx.update(|_, cx| kit::Tk::of(cx))
}

/// The first shadow is `ink` at `spread`, inset: the field's one ring.
fn assert_shadow(p: paint_probe::Painted, ink: gpui::Hsla, spread: f32, what: &str) {
    assert_eq!(p.shadow, Some(paint_probe::hsla24(ink)), "{what}: ring ink");
    // RGB alone cannot tell a solid ring from the page's semi-transparent
    // `k15`/`k45` ink; the alpha the box carried is part of the ink.
    let alpha = p.shadow_alpha.expect("the field has a shadow");
    assert!(
        (alpha - ink.a).abs() < 1e-3,
        "{what}: ring alpha {alpha} vs {}",
        ink.a
    );
    assert_eq!(p.shadow_inset, Some(true), "{what}: an inset ring");
    let got = p.shadow_spread.expect("the field has a shadow");
    assert!(
        (got - spread).abs() < 1e-3,
        "{what}: spread {got} vs {spread}"
    );
}

fn focus(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, input: &Entity<InputState>) {
    use gpui::Focusable as _;
    let handle = app.update_in(vcx, |_, window, cx| {
        input.update(cx, |i, cx| i.focus(window, cx));
        input.read(cx).focus_handle(cx)
    });
    settle_fresh(vcx);
    assert!(
        vcx.update(|w, _| handle.is_focused(w)),
        "setup: the field's own Input holds focus"
    );
}

fn blur(vcx: &mut VisualTestContext) {
    vcx.update(|w, _| w.blur());
    settle_fresh(vcx);
}

fn set(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, input: &Entity<InputState>, v: &str) {
    app.update_in(vcx, |_, window, cx| {
        input.update(cx, |i, cx| i.set_value(v.to_string(), window, cx))
    });
    settle_fresh(vcx);
}

fn value(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    input: &Entity<InputState>,
) -> String {
    app.update_in(vcx, |_, _, cx| input.read(cx).value().to_string())
}

fn near(a: gpui::Bounds<gpui::Pixels>, b: gpui::Bounds<gpui::Pixels>) -> bool {
    let close = |x: gpui::Pixels, y: gpui::Pixels| (x - y).abs() <= px(0.5);
    close(a.origin.x, b.origin.x)
        && close(a.origin.y, b.origin.y)
        && close(a.size.width, b.size.width)
        && close(a.size.height, b.size.height)
}

fn gap(vcx: &mut VisualTestContext) -> gpui::Pixels {
    vcx.update(|_, cx| {
        use gpui_component::ActiveTheme as _;
        cx.theme().focus_ring_offset()
    })
}

/// `ring` is `field` moved in by the installed gap on every side, and inside it.
fn assert_inside_ring(
    vcx: &mut VisualTestContext,
    field: gpui::Bounds<gpui::Pixels>,
    ring: gpui::Bounds<gpui::Pixels>,
) {
    let by = gap(vcx);
    let want = gpui::Bounds {
        origin: gpui::point(field.origin.x + by, field.origin.y + by),
        size: size(field.size.width - by * 2.0, field.size.height - by * 2.0),
    };
    assert!(
        near(ring, want),
        "PAINT: the ring {ring:?} is the field {field:?} moved in by the installed gap {by:?}"
    );
    assert!(
        ring.top() >= field.top()
            && ring.bottom() <= field.bottom()
            && ring.left() >= field.left()
            && ring.right() <= field.right(),
        "PAINT: the ring {ring:?} lies inside the field {field:?}"
    );
}

/// The host form: a valid Host, an invalid Port, and its two secrets.
struct HostForm {
    app: Entity<Tty59App>,
    vcx: VisualTestContext,
    host: Entity<InputState>,
    port: Entity<InputState>,
    password: Entity<InputState>,
}

fn host_form(cx: &mut TestAppContext) -> HostForm {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Ssh, window, cx);
        app.add_new_profile(window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1400.)));
    settle(&mut vcx);
    let (host, port, password) = app.update_in(&mut vcx, |app, _, _| {
        let f = app
            .active_settings()
            .and_then(|s| s.ssh_form.as_ref())
            .expect("the new-host form is open");
        (f.host.clone(), f.port.clone(), f.password.clone())
    });
    set(&app, &mut vcx, &host, "box.example");
    set(&app, &mut vcx, &port, "abc");
    HostForm {
        app,
        vcx,
        host,
        port,
        password,
    }
}

// ------------------------------------------------------------------ what already works

/// f0: a valid, unfocused field is a 26px box with the hairline ring and no
/// overlay.
#[gpui::test]
fn f0_a_valid_unfocused_field_is_a_26px_box_with_the_hairline_ring(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut h = host_form(cx);
    let id = id_of(&h.host);
    settle_fresh(&mut h.vcx);
    let b = bounds_of(&mut h.vcx, id).expect("the host field box is drawn");
    assert_eq!(b.size.height, px(26.), "the box height");
    let k = tk(&mut h.vcx);
    assert_shadow(painted(id), k.k15, kit::hairline(), "valid, unfocused");
    assert!(
        ring_of(&mut h.vcx, id).is_none(),
        "no overlay while unfocused"
    );
}

/// f3: an invalid, unfocused field shows the danger ring and no overlay.
#[gpui::test]
fn f3_an_invalid_unfocused_field_shows_the_danger_ring(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut h = host_form(cx);
    let id = id_of(&h.port);
    settle_fresh(&mut h.vcx);
    let k = tk(&mut h.vcx);
    assert_shadow(painted(id), k.danger, 1., "invalid port, unfocused");
    assert!(
        ring_of(&mut h.vcx, id).is_none(),
        "no overlay while unfocused"
    );
}

/// f5: focusing and blurring a field moves and resizes nothing, valid or invalid.
#[gpui::test]
fn f5_focusing_a_field_never_changes_its_box(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut h = host_form(cx);
    for (name, input) in [
        ("valid host", h.host.clone()),
        ("invalid port", h.port.clone()),
    ] {
        let id = id_of(&input);
        settle_fresh(&mut h.vcx);
        let before = bounds_of(&mut h.vcx, id).expect("drawn");
        focus(&h.app, &mut h.vcx, &input);
        let during = bounds_of(&mut h.vcx, id).expect("drawn");
        blur(&mut h.vcx);
        let after = bounds_of(&mut h.vcx, id).expect("drawn");
        assert_eq!(during, before, "{name}: focused box vs unfocused");
        assert_eq!(after, before, "{name}: blurred box vs unfocused");
    }
}

/// a1: the Arguments field with an unmatched quote is invalid, draws the page's
/// real explanation by the same entity, and a valid value draws none.
#[gpui::test]
fn a1_an_arguments_field_with_an_open_quote_shows_its_error_line(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Terminal, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    let args = app.update_in(&mut vcx, |app, _, _| {
        app.active_settings().unwrap().shell_args_input.clone()
    });
    let id = id_of(&args);
    set(&app, &mut vcx, &args, "--ok");
    assert!(
        paint_probe::label(&format!("settings-field-{id}-error")).is_none(),
        "a valid value draws no explanation"
    );
    let k = tk(&mut vcx);
    assert_shadow(painted(id), k.k15, kit::hairline(), "valid arguments");

    set(&app, &mut vcx, &args, "'");
    let label = paint_probe::label(&format!("settings-field-{id}-error"))
        .expect("an open quote draws the page's explanation");
    assert!(!label.is_empty(), "the explanation has text");
    let line = vcx
        .debug_bounds(leak(format!("settings-field-{id}-error")))
        .expect("and its line is laid out");
    assert!(line.size.height > px(0.) && line.size.width > px(0.));
    assert_shadow(painted(id), k.danger, 1., "invalid arguments");
}

// ------------------------------------------------------------------ the overlay (RED today)

/// f1: a focused valid field keeps its `k45` ring AND draws the overlay inside it.
#[gpui::test]
fn f1_a_focused_field_draws_the_ring_inside_its_box(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut h = host_form(cx);
    let id = id_of(&h.host);
    focus(&h.app, &mut h.vcx, &h.host.clone());
    let k = tk(&mut h.vcx);
    assert_shadow(
        painted(id),
        k.k45,
        1.,
        "valid, focused (the existing ring stays)",
    );
    let field = bounds_of(&mut h.vcx, id).expect("the field box");
    let ring = ring_of(&mut h.vcx, id).expect("PAINT: a focused field draws no overlay ring");
    assert_inside_ring(&mut h.vcx, field, ring);
}

/// f2: focus by a real click on the field draws the same overlay.
#[gpui::test]
fn f2_a_click_that_focuses_a_field_draws_the_ring_too(cx: &mut TestAppContext) {
    use gpui::Focusable as _;
    let _probe = paint_probe::scope();
    let mut h = host_form(cx);
    let id = id_of(&h.host);
    let at = bounds_of(&mut h.vcx, id).expect("the field box").center();
    h.vcx.simulate_click(at, Modifiers::none());
    settle_fresh(&mut h.vcx);
    let handle = h
        .app
        .update_in(&mut h.vcx, |_, _, cx| h.host.read(cx).focus_handle(cx));
    assert!(
        h.vcx.update(|w, _| handle.is_focused(w)),
        "FOCUS: the click focused the field's own Input"
    );
    let field = bounds_of(&mut h.vcx, id).expect("the field box");
    let ring = ring_of(&mut h.vcx, id).expect("PAINT: a click-focused field draws no overlay ring");
    assert_inside_ring(&mut h.vcx, field, ring);
}

/// f4: an invalid AND focused field has both: the danger ring (not swapped for
/// `k45`) and the overlay, in a box that did not change.
#[gpui::test]
fn f4_an_invalid_focused_field_keeps_the_danger_ring_and_draws_the_overlay(
    cx: &mut TestAppContext,
) {
    let _probe = paint_probe::scope();
    let mut h = host_form(cx);
    let id = id_of(&h.port);
    settle_fresh(&mut h.vcx);
    let before = bounds_of(&mut h.vcx, id).expect("the field box");
    focus(&h.app, &mut h.vcx, &h.port.clone());
    let k = tk(&mut h.vcx);
    assert_shadow(
        painted(id),
        k.danger,
        1.,
        "invalid, focused: still the danger ring",
    );
    let field = bounds_of(&mut h.vcx, id).expect("the field box");
    assert_eq!(field, before, "focus did not change the box");
    let ring = ring_of(&mut h.vcx, id).expect("PAINT: an invalid focused field draws no overlay");
    assert_inside_ring(&mut h.vcx, field, ring);
}

/// f6: typing into a focused field edits it and leaves the ring, the shadow and
/// the box alone: valid host, and the invalid Arguments field (whose error line
/// stays drawn).
#[gpui::test]
fn f6_typing_into_a_focused_field_leaves_its_indication_alone(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut h = host_form(cx);
    let id = id_of(&h.host);
    set(&h.app, &mut h.vcx, &h.host.clone(), "");
    focus(&h.app, &mut h.vcx, &h.host.clone());
    let before = bounds_of(&mut h.vcx, id).expect("the field box");
    h.vcx.simulate_input("ab");
    settle_fresh(&mut h.vcx);
    assert_eq!(
        value(&h.app, &mut h.vcx, &h.host.clone()),
        "ab",
        "the Input edits"
    );
    let k = tk(&mut h.vcx);
    assert_shadow(painted(id), k.k45, 1., "typing, valid");
    assert_eq!(
        bounds_of(&mut h.vcx, id),
        Some(before),
        "the box is unchanged"
    );
    let ring = ring_of(&mut h.vcx, id).expect("PAINT: the ring is still drawn while typing");
    assert_inside_ring(&mut h.vcx, before, ring);
}

#[gpui::test]
fn f6b_typing_into_an_invalid_arguments_field_keeps_danger_ring_overlay_and_error(
    cx: &mut TestAppContext,
) {
    let _probe = paint_probe::scope();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Terminal, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    let args = app.update_in(&mut vcx, |app, _, _| {
        app.active_settings().unwrap().shell_args_input.clone()
    });
    let id = id_of(&args);
    set(&app, &mut vcx, &args, "'");
    focus(&app, &mut vcx, &args);
    let before = bounds_of(&mut vcx, id).expect("the field box");
    vcx.simulate_input("x");
    settle_fresh(&mut vcx);
    let v = value(&app, &mut vcx, &args);
    assert!(
        v.len() == 2 && v.contains('x') && v.contains('\''),
        "the Input edits: {v:?}"
    );
    let k = tk(&mut vcx);
    assert_shadow(painted(id), k.danger, 1., "typing, invalid");
    assert_eq!(
        bounds_of(&mut vcx, id),
        Some(before),
        "the box is unchanged"
    );
    assert!(
        paint_probe::label(&format!("settings-field-{id}-error")).is_some_and(|l| !l.is_empty()),
        "the error line is still drawn"
    );
    let ring = ring_of(&mut vcx, id).expect("PAINT: the ring is still drawn while typing");
    assert_inside_ring(&mut vcx, before, ring);
}

/// f7: the secret (password) field draws the same.
#[gpui::test]
fn f7_a_secret_field_draws_the_ring_too(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut h = host_form(cx);
    let id = id_of(&h.password);
    settle_fresh(&mut h.vcx);
    let k = tk(&mut h.vcx);
    assert_shadow(painted(id), k.k15, kit::hairline(), "secret, unfocused");
    focus(&h.app, &mut h.vcx, &h.password.clone());
    assert_shadow(painted(id), k.k45, 1., "secret, focused");
    let field = bounds_of(&mut h.vcx, id).expect("the field box");
    let ring = ring_of(&mut h.vcx, id).expect("PAINT: a focused secret field draws no overlay");
    assert_inside_ring(&mut h.vcx, field, ring);
}

/// f8: only the focused Input has an overlay.
#[gpui::test]
fn f8_only_the_focused_field_has_an_overlay(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut h = host_form(cx);
    let (host, port) = (id_of(&h.host), id_of(&h.port));
    focus(&h.app, &mut h.vcx, &h.host.clone());
    assert!(
        ring_of(&mut h.vcx, port).is_none(),
        "the unfocused port has no overlay"
    );
    assert!(
        ring_of(&mut h.vcx, host).is_some(),
        "PAINT: the focused host draws its overlay"
    );
}
