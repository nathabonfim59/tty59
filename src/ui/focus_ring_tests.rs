//! Tests for the keyboard focus ring's theme role (A2, owner decisions 4 and 5).
//!
//! The ring floats `FOCUS_RING_GAP` (2px) off a toolkit control, so the pair that
//! counts is the ring against the surface behind it, at least 3:1 (a component, not
//! text). `Theme::focus_ring_ink_from` derives that ink once per theme change and
//! `apply_theme` installs it on the gpui `Theme`, which is the cache.
//!
//! What is read, and from where:
//!
//! * The fills the ring is held against are read from what `apply_theme` actually
//!   installed (the `Surfaces` global, `Theme::secondary` and `Theme::muted`), not from
//!   the derivation's own list, so dropping a set from the derivation shows up here.
//! * The installed ink is compared with the pure derivation for every theme, so a
//!   missing or stale hookup shows up too.
//! * The painted witness draws real focused toolkit `Button`s in a headless window and
//!   reads the painted `Scene::quads`. Its recorder is a copy of the one in
//!   gpui-component's `button/focus_ring_tests.rs` and lives only in this test file;
//!   there is no runtime hook. The headless platform's window has a fixed scale factor
//!   of 2, so the painted checks run at 2x only; every expectation is written in
//!   logical pixels times `scale_factor()` and nothing here claims 1x. Native 1x and 2x
//!   captures remain mandatory and are taken on Boat.
//! * The 15 fills the ring floats over are read back, not derived here. The one fill the
//!   field-edge sweep below does compute is a field's INSIDE fill: the toolkit's installed
//!   `input_background()` composited over each of them, by a copy of the renderer's "over"
//!   written in this file (`over_encoded`), independent of the production one in `presets`.
//! * The SCM commit frame's committable (inverted) fill is not
//!   one of the fills: that state draws an inset ring in the panel colour instead.
//!   Its not-committable fill, `field_fill`, is the midpoint of the sidebar's base and
//!   hover rungs and is covered by the segment proof below.
//!
//! Floors are written out here rather than read from `presets`, so lowering the
//! production constant cannot make these pass.

use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use gpui::{
    AnyWindowHandle, App, AppContext as _, Background, Context, DevicePixels, HeadlessAppContext,
    Hsla, IntoElement, NoopTextSystem, ParentElement as _, PlatformAtlas, PlatformHeadlessRenderer,
    Quad, Render, Scene, Size, Styled as _, TestAppContext, Window, div, px, rgb, size,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Selectable as _,
    button::{Button, ButtonVariants as _},
};

use crate::core::config::{Config, CoreConfig};
use crate::ui::paint_probe::hsla24;
use crate::ui::presets::{self, Surfaces, contrast};
use crate::ui::tab_strip::chrome_tile_variant;
use crate::ui::theme::apply_theme;

/// The inverted-neutral custom variant the v4 primary button used before A3's
/// Primary recipe replaced it, kept here byte for byte as the historical
/// fixture for the `Variant::Inverted` witness: body ink as the fill, `surface`
/// as the label. It is not the app's primary button any more and does not test
/// the A3 Primary recipe (`dialog::primary_button`); that has its own tests.
fn inverted_button(surface: Hsla, cx: &App) -> gpui_component::button::ButtonCustomVariant {
    let ink = cx.theme().foreground;
    gpui_component::button::ButtonCustomVariant::new(cx)
        .color(ink)
        .foreground(surface)
        .hover(ink.blend(surface.opacity(0.14)))
        .active(ink.blend(surface.opacity(0.24)))
}

/// The component floor the ring must clear on every fill it can float over.
const FLOOR: f32 = 3.0;
/// The gap between a control and its ring, in logical pixels.
const GAP: f32 = 2.0;
/// Width of the ring border drawn by the toolkit.
const RING_WIDTH: f32 = 1.5;
const BUTTON_ID: &str = "focus-ring-witness";
/// Host padding around the button, and the fixed size the button is given.
const PAD: f32 = 20.;
const BUTTON_W: f32 = 96.;
const BUTTON_H: f32 = 32.;

/// Every theme the app derives chrome for: the 13 built-ins and the 864 generated palettes.
fn all_themes() -> Vec<presets::Theme> {
    let mut themes = presets::builtins();
    themes.extend(presets::generated_theme_grid());
    themes
}

/// Installs `theme` through the real `apply_theme`, as a theme change does.
fn install(cx: &mut App, theme: &presets::Theme) {
    cx.set_global(Config(CoreConfig {
        theme_follow_system: false,
        theme_preset: theme.id.clone(),
        ..Default::default()
    }));
    apply_theme(None, cx);
}

/// The solid fills the ring can float over, read from what `apply_theme` installed:
/// every rung of the four surfaces, the page, and the two neutral wells.
fn installed_fills(cx: &App, theme: &presets::Theme) -> Vec<(String, u32)> {
    let sf = cx.global::<Surfaces>();
    let mut fills = Vec::new();
    for (set, surface) in [
        ("window", sf.window),
        ("sidebar", sf.sidebar),
        ("rail", sf.rail),
        ("popover", sf.popover),
    ] {
        for (rung, fill) in [
            ("base", surface.base),
            ("hover", surface.hover),
            ("selected", surface.selected),
        ] {
            fills.push((format!("{set}.{rung}"), fill));
        }
    }
    fills.push(("page".to_string(), theme.background_color()));
    fills.push(("secondary".to_string(), hsla24(cx.theme().secondary)));
    fills.push(("muted".to_string(), hsla24(cx.theme().muted)));
    fills
}

fn assert_no_misses(what: &str, misses: Vec<String>) {
    assert!(
        misses.is_empty(),
        "{what}: {} misses, first 12:\n{}",
        misses.len(),
        misses
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The sweep reads colours back through `Hsla`; this holds the conversion exact so a
/// one-bit rounding cannot decide a 3:1 verdict.
#[test]
fn hsla24_round_trips_every_8_bit_value_of_each_channel_and_grey() {
    for v in 0u32..=255 {
        for shift in [16, 8, 0] {
            let colour = v << shift;
            assert_eq!(hsla24(Hsla::from(rgb(colour))), colour, "{colour:06x}");
        }
        let grey = v * 0x01_01_01;
        assert_eq!(hsla24(Hsla::from(rgb(grey))), grey, "grey {grey:06x}");
    }
}

/// The installed ring ink clears 3:1 on every installed fill, in all 877 themes, is
/// opaque, and equals the accent seed wherever the seed already clears the union.
#[gpui::test]
fn the_installed_ring_clears_every_surface_it_can_float_over_in_all_877_themes(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    let themes = all_themes();
    assert_eq!(themes.len(), 877, "13 built-ins and 864 generated palettes");
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(presets::Themes(themes.clone()));
        let mut misses = Vec::new();
        let mut moved = 0usize;
        let mut kept = 0usize;
        for theme in &themes {
            install(cx, theme);
            let Some(ring) = cx.theme().focus_ring else {
                misses.push(format!("{}: no focus ring installed", theme.id));
                continue;
            };
            if (ring.a - 1.0).abs() > 1e-6 {
                misses.push(format!("{}: ring alpha {} is not opaque", theme.id, ring.a));
            }
            let ink = hsla24(ring);
            for (name, fill) in installed_fills(cx, theme) {
                let ratio = contrast(ink, fill);
                if ratio < FLOOR {
                    misses.push(format!(
                        "{}: ring {ink:06x} on {name} {fill:06x} is only {ratio:.2}:1",
                        theme.id
                    ));
                }
            }
            // The seed is the accent `ring` has always been. It stays untouched where it
            // already clears the union, and moves where it does not.
            let seed = hsla24(cx.theme().ring);
            let resolved = theme.resolve();
            // The union now includes the field wash inside a field's focused edge: read from
            // the theme `apply_theme` installed, as the toolkit's getter returns it.
            let wash = cx.theme().input_background();
            let seed_clears = theme
                .focus_ring_field_fills(&resolved, wash)
                .iter()
                .all(|&fill| contrast(seed, fill) >= FLOOR);
            match (seed_clears, ink == seed) {
                (true, true) => kept += 1,
                (false, false) => moved += 1,
                (true, false) => misses.push(format!(
                    "{}: seed {seed:06x} already clears the union but the ink moved to {ink:06x}",
                    theme.id
                )),
                (false, true) => misses.push(format!(
                    "{}: seed {seed:06x} fails the union but the ink did not move",
                    theme.id
                )),
            }
        }
        assert_no_misses("installed ring ink vs installed fills", misses);
        // Every theme is classified exactly once; the counts are printed for the report.
        println!("ring ink kept the accent in {kept} themes and moved it in {moved}");
        assert_eq!(kept + moved, themes.len(), "every theme is kept or moved");
    });
}

/// The wash a field paints over its backdrop, composited the way the renderer does: a straight
/// "over" on the sRGB-ENCODED channels (the surface format is not sRGB, so there is no
/// linearisation), `wash.rgb * wash.a + surface * (1 - wash.a)`, rounded to 8 bits. The RGB and alpha
/// are read from the `Hsla` as the toolkit returns it, through the conversion `hsla24` itself uses, so
/// nothing about the wash (its alpha, its colour) is assumed here.
fn over_encoded(wash: Hsla, surface: u32) -> u32 {
    let w = gpui::Rgba::from(wash);
    let channel = |wash_channel: f32, shift: u32| -> u32 {
        let under = ((surface >> shift) & 0xff) as f32 / 255.0;
        ((wash_channel * w.a + under * (1.0 - w.a)) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u32
    };
    channel(w.r, 16) << 16 | channel(w.g, 8) << 8 | channel(w.b, 0)
}

/// The focused edge of a toolkit field (proposal A of the toolkit's `focused_border`) is the installed ring
/// ink, opaque, 1px wide. It touches two fills: the one OUTSIDE it (the surface the field sits on) and the
/// one INSIDE it (the toolkit's `input_background()`, read from the installed theme as the `Hsla` it
/// returns, composited over that surface). Both must clear the 3:1 component floor, in all 877 themes.
///
/// The outside set is the 15 installed fills of the 877-theme sweep above; the inside set is the installed
/// wash over each of them (a field can sit on any of them: a superset of where the app places fields, not
/// a map of it). The report is printed BEFORE the assertions so a red run still carries the numbers.
/// Modelled: opaque default config (no window translucency), the encoded-sRGB "over", 8-bit rounding.
/// NOT modelled: window translucency, anti-aliasing and the corner radius of the 1px edge, a disabled
/// field, an open `Select`'s outline, or any backdrop that is not one of the 15 fills.
///
/// This is a hard oracle: any outside or inside miss, in any theme, FAILS it (the ink derivation is
/// pole-limited, so a theme it cannot clear is reported and fails, never accepted). The report lists
/// every theme with a miss before the assertions run.
///
/// The `FIELD-EDGE MAP` lines are a SOURCE-BACKED mapping of which fill each control kind paints inside
/// its focused edge, read from the toolkit at `0280dc24`; they are not a claim that four scene hosts
/// were rendered here, and each repeats the sweep's own inside result because the enabled fill is the
/// same installed getter for all four.
#[gpui::test]
fn the_focused_field_edge_clears_both_fills_it_touches_in_all_877_themes(cx: &mut TestAppContext) {
    use std::collections::{BTreeMap, BTreeSet};
    crate::core::config::pin_test_config_dir();
    let themes = all_themes();
    assert_eq!(themes.len(), 877, "13 built-ins and 864 generated palettes");
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(presets::Themes(themes.clone()));
        let mut miss_themes: BTreeSet<String> = BTreeSet::new();
        let mut structural = Vec::new();
        let mut outside_misses = Vec::new();
        let mut inside_misses = Vec::new();
        let mut inside_all: Vec<(f32, String)> = Vec::new();
        let mut outside_min = f32::MAX;
        let mut inside_min = f32::MAX;
        let mut wash_alpha_min = f32::MAX;
        let mut wash_alpha_max = f32::MIN;
        let mut translucent_washes = 0usize;
        let mut classes: BTreeMap<String, (usize, f32)> = BTreeMap::new();
        let mut measured = 0usize;
        let mut stock_ink: Vec<String> = Vec::new();
        let mut ink_moves: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        let mut light_changed: Vec<String> = Vec::new();
        for theme in &themes {
            install(cx, theme);
            let Some(ring) = cx.theme().focus_ring else {
                structural.push(format!("{}: no focus ring installed", theme.id));
                continue;
            };
            if (ring.a - 1.0).abs() > 1e-6 {
                structural.push(format!("{}: ring alpha {} is not opaque", theme.id, ring.a));
                continue;
            }
            let wash = cx.theme().input_background();
            if !(wash.a > 0.0 && wash.a <= 1.0) {
                structural.push(format!(
                    "{}: the installed input background has alpha {}",
                    theme.id, wash.a
                ));
                continue;
            }
            let fills = installed_fills(cx, theme);
            if fills.len() != 15 {
                structural.push(format!(
                    "{}: {} installed fills, expected the 15 of the ring sweep",
                    theme.id,
                    fills.len()
                ));
                continue;
            }
            measured += 1;
            wash_alpha_min = wash_alpha_min.min(wash.a);
            wash_alpha_max = wash_alpha_max.max(wash.a);
            if wash.a < 1.0 {
                translucent_washes += 1;
            }
            let ink = hsla24(ring);
            // The ink the ring had before the field fills joined the union, for the old/new
            // report: the pure derivation over the outside fills alone.
            let old_ink = theme.focus_ring_ink_outside_only(&theme.resolve());
            let changed = old_ink != ink;
            if theme.id == "dark" || theme.id == "light" {
                stock_ink.push(format!(
                    "FIELD-EDGE SWEEP: stock {}: old ink {old_ink:06x}, new ink {ink:06x}",
                    theme.id
                ));
            }
            let moves = ink_moves
                .entry(if theme.dark { "dark" } else { "light" })
                .or_insert((0usize, 0usize));
            moves.0 += 1;
            if changed {
                moves.1 += 1;
            }
            if !theme.dark && wash.a == 1.0 && changed {
                light_changed.push(format!(
                    "{}: light theme, opaque wash, old ink {old_ink:06x}, new ink {ink:06x}",
                    theme.id
                ));
            }
            let kept = ink == hsla24(cx.theme().ring);
            let class = format!(
                "{}/{}",
                if theme.dark { "dark" } else { "light" },
                if kept { "seed kept" } else { "seed moved" }
            );
            let mut theme_min = f32::MAX;
            for (name, fill) in &fills {
                let outside = contrast(ink, *fill);
                outside_min = outside_min.min(outside);
                if outside < FLOOR {
                    miss_themes.insert(theme.id.clone());
                    outside_misses.push(format!(
                        "{}: ring {ink:06x} outside, on {name} {fill:06x}, is only {outside:.2}:1",
                        theme.id
                    ));
                }
                let inside_fill = over_encoded(wash, *fill);
                let inside = contrast(ink, inside_fill);
                inside_min = inside_min.min(inside);
                theme_min = theme_min.min(inside);
                inside_all.push((
                    inside,
                    format!(
                        "{}: ring {ink:06x} inside, wash over {name} {fill:06x} = {inside_fill:06x}, {inside:.2}:1",
                        theme.id
                    ),
                ));
                if inside < FLOOR {
                    miss_themes.insert(theme.id.clone());
                    inside_misses.push(format!(
                        "{}: ring {ink:06x} inside, wash over {name} {fill:06x} = {inside_fill:06x}, is only {inside:.2}:1",
                        theme.id
                    ));
                }
            }
            let entry = classes.entry(class).or_insert((0, f32::MAX));
            entry.0 += 1;
            entry.1 = entry.1.min(theme_min);
        }
        inside_all.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        // The stock themes first: if their ink moved, every ring frame already captured on them
        // is stale.
        for line in &stock_ink {
            println!("{line}");
        }
        for (side, (total, changed)) in &ink_moves {
            println!(
                "FIELD-EDGE SWEEP: {side} themes: {total} measured, ink changed from the outside-only derivation in {changed}"
            );
        }
        println!(
            "FIELD-EDGE SWEEP: {measured} of {} themes measured, 15 outside fills each, inside = installed input_background() over each",
            themes.len()
        );
        println!(
            "FIELD-EDGE SWEEP: wash alpha min {wash_alpha_min:.4} max {wash_alpha_max:.4}, translucent washes in {translucent_washes} themes"
        );
        println!(
            "FIELD-EDGE SWEEP: outside misses {} (min margin {outside_min:.3}:1), inside misses {} (min margin {inside_min:.3}:1), floor {FLOOR}:1",
            outside_misses.len(),
            inside_misses.len()
        );
        println!(
            "FIELD-EDGE SWEEP: themes with an outside or inside miss: {}",
            miss_themes.len()
        );
        if !miss_themes.is_empty() {
            println!(
                "FIELD-EDGE SWEEP: themes with a miss: {}",
                miss_themes.iter().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        for (class, (count, min)) in &classes {
            println!("FIELD-EDGE SWEEP: class {class}: {count} themes, minimum inside margin {min:.3}:1");
        }
        for (_, line) in inside_all.iter().take(12) {
            println!("FIELD-EDGE SWEEP: tightest inside: {line}");
        }
        println!(
            "FIELD-EDGE SWEEP: not modelled: window translucency, edge anti-aliasing and corner radius, disabled fields, an open Select, backdrops outside the 15 fills"
        );
        // Which fill each control kind paints inside its focused edge, from the toolkit source at
        // 0280dc24 (read, not rendered here). Every enabled kind takes it from `input_style`, i.e.
        // the same installed `input_background()` this sweep composites, so each line repeats the
        // sweep's own inside result.
        let kinds: [(&str, &str); 4] = [
            (
                "Input",
                "input/input.rs: enabled fill = input_style(false).0 = theme.input_background() (22-30, used at 268); code-editor mode swaps in editor_background() (269-272, not a bordered app Input); disabled fill = input.mix_oklab(transparent, 0.8) at 0.5 opacity (274) but the edge is gated by focused = is_focused && !disabled (261); the edge is drawn only inside .when(appearance).when(bordered).when(focused && focus_bordered) (527-536), appearance defaults to true; no hover or active fill override",
            ),
            (
                "Select",
                "select.rs: fill = input_style(disabled).0 (471), the same input_background() when enabled, applied inside .when(appearance) at 491; the edge is OUTSIDE the appearance block at 509, .when(outline_visible) with outline_visible = open || (is_focused && !disabled) (463): an open Select shows the edge on the same fill; a closed disabled Select has no focused edge, an already-open disabled state is outside this ENABLED-fill sweep; no hover or active fill override on the trigger; the app constructs no Select",
            ),
            (
                "Combobox",
                "combobox.rs: fill = input_style(disabled).0 (575) passed to the trigger helper and applied at 927 inside .when(appearance); the edge is at 939, .when(outline_visible) with outline_visible = open || (is_focused && !disabled) (572): same fill; a closed disabled Combobox has no focused edge, an already-open disabled state is outside this ENABLED-fill sweep; the app constructs no Combobox",
            ),
            (
                "DatePicker",
                "time/date_picker.rs: fill = input_style(self.disabled).0 (381) applied at 405 inside .when(self.appearance), at 0.5 opacity when disabled; the edge is at 412, .when(is_focused) and NOT gated by disabled (the known quirk, exempt by order: a disabled picker holding focus would draw the edge over input.mix_oklab(transparent, 0.8) at 0.5, which this sweep does not cover); the app constructs no DatePicker",
            ),
        ];
        for (kind, source) in kinds {
            println!(
                "FIELD-EDGE MAP {kind}: {source}; sweep result for the enabled fill: {} inside misses, minimum inside margin {inside_min:.3}:1",
                inside_misses.len()
            );
        }
        assert_no_misses("field edge: structure", structural);
        assert_eq!(measured, themes.len(), "every theme was measured");
        assert_no_misses(
            "field edge: a light theme with an opaque wash changed its ink",
            light_changed,
        );
        assert_no_misses("field edge ring vs the fill outside it", outside_misses);
        assert_no_misses("field edge ring vs the wash fill inside it", inside_misses);
    });
}

/// The installed ring is exactly the pure derivation, with the 2px gap, and follows
/// every theme change (built-in, light, dark, generated) without keeping an older value.
#[gpui::test]
fn the_installed_ring_is_the_derivation_and_follows_every_theme_change(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let mut themes = all_themes();
    // Alternate dark and light so a stale value from the previous theme would differ.
    themes.sort_by_key(|theme| (theme.dark, theme.id.clone()));
    let mut order = Vec::new();
    let (dark, light): (Vec<_>, Vec<_>) = themes.iter().cloned().partition(|theme| theme.dark);
    for i in 0..dark.len().max(light.len()) {
        order.extend(dark.get(i).cloned());
        order.extend(light.get(i).cloned());
    }
    assert_eq!(order.len(), 877);
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(presets::Themes(themes.clone()));
        let mut distinct = Vec::new();
        for theme in &order {
            install(cx, theme);
            let resolved = theme.resolve();
            let wash = cx.theme().input_background();
            let want: Hsla = rgb(theme.focus_ring_ink_from(&resolved, wash)).into();
            assert_eq!(
                cx.theme().focus_ring,
                Some(want),
                "{}: the installed ring is not the derivation",
                theme.id
            );
            assert_eq!(
                cx.theme().focus_ring_offset,
                px(GAP),
                "{}: the installed offset is not 2px",
                theme.id
            );
            assert_eq!(cx.theme().focus_ring_offset(), px(GAP), "{}", theme.id);
            // `ring` keeps its accent for the input border and the switch.
            assert_eq!(
                hsla24(cx.theme().ring),
                resolved.m.accent,
                "{}: the accent ring changed",
                theme.id
            );
            let ink = hsla24(want);
            if !distinct.contains(&ink) {
                distinct.push(ink);
            }
        }
        assert!(
            distinct.len() > 8,
            "only {} distinct inks across 877 themes: staleness would not show",
            distinct.len()
        );
    });
}

/// Per-channel order of `a` to `b`: every channel rises or stays, or every channel
/// falls or stays.
fn co_monotone(a: u32, b: u32) -> bool {
    let delta = |shift: u32| ((b >> shift & 0xff) as i32) - ((a >> shift & 0xff) as i32);
    let deltas = [delta(16), delta(8), delta(0)];
    deltas.iter().all(|&d| d >= 0) || deltas.iter().all(|&d| d <= 0)
}

/// Luminance order, by contrast against black (it grows with luminance, so the
/// production luminance formula is not copied here).
fn rank(colour: u32) -> f32 {
    contrast(colour, 0x00_00_00)
}

fn same_side(ink: u32, a: u32, b: u32) -> bool {
    let (ink, a, b) = (rank(ink), rank(a), rank(b));
    (ink >= a && ink >= b) || (ink <= a && ink <= b)
}

/// A fill that is a blend between two members of the union (a hover wash, the SCM
/// message box's `field_fill`, which is the midpoint of the sidebar's base and hover
/// rungs) is covered by its two ends, provided the channels move together (so luminance
/// is monotone along the blend) and the ink is on one side of both ends (so the contrast
/// is monotone too). This holds that for every adjacent pair of rungs and for each
/// neutral well against the page, over all 877 themes.
#[gpui::test]
fn every_blend_between_union_members_is_covered_by_its_two_ends(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let themes = all_themes();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(presets::Themes(themes.clone()));
        let mut misses = Vec::new();
        let mut segments = 0usize;
        for theme in &themes {
            install(cx, theme);
            let ink = hsla24(cx.theme().focus_ring.expect("a ring is installed"));
            let sf = cx.global::<Surfaces>().clone();
            let page = theme.background_color();
            let mut pairs: Vec<(String, u32, u32)> = Vec::new();
            for (set, surface) in [
                ("window", sf.window),
                ("sidebar", sf.sidebar),
                ("rail", sf.rail),
                ("popover", sf.popover),
            ] {
                pairs.push((format!("{set}.base->hover"), surface.base, surface.hover));
                pairs.push((
                    format!("{set}.hover->selected"),
                    surface.hover,
                    surface.selected,
                ));
            }
            pairs.push((
                "page->secondary".to_string(),
                page,
                hsla24(cx.theme().secondary),
            ));
            pairs.push(("page->muted".to_string(), page, hsla24(cx.theme().muted)));
            for (name, a, b) in pairs {
                segments += 1;
                if contrast(ink, a) < FLOOR || contrast(ink, b) < FLOOR {
                    // The ends are held by the sweep above; a segment is only meaningful
                    // once both of its ends clear.
                    continue;
                }
                if !co_monotone(a, b) {
                    misses.push(format!(
                        "{}: {name} {a:06x}->{b:06x} does not move its channels together",
                        theme.id
                    ));
                } else if !same_side(ink, a, b) {
                    misses.push(format!(
                        "{}: ring {ink:06x} is between the ends of {name} {a:06x}->{b:06x}",
                        theme.id
                    ));
                }
            }
        }
        assert_no_misses("blends between union members", misses);
        assert!(segments > 877 * 8, "only {segments} segments were checked");
    });
}

// ---------------------------------------------------------------------------
// The painted witness. The adapter below is copied from gpui-component's
// `button/focus_ring_tests.rs` and is test-only: it keeps the quads of the last drawn
// frame and paints nothing.
// ---------------------------------------------------------------------------

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
    /// Fails when the log is poisoned: a stale scene must never be read back as if it
    /// were the frame just drawn.
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

/// The button variants the app puts a ring on: the toolkit's own, the inverted primary
/// (the Agents pill and its kin) and the title-bar tile.
#[derive(Clone, Copy, Debug)]
enum Variant {
    Default,
    Primary,
    Secondary,
    Danger,
    Ghost,
    Selected,
    Inverted,
    ChromeTile,
}

const VARIANTS: [Variant; 8] = [
    Variant::Default,
    Variant::Primary,
    Variant::Secondary,
    Variant::Danger,
    Variant::Ghost,
    Variant::Selected,
    Variant::Inverted,
    Variant::ChromeTile,
];

impl Variant {
    fn button(self, button: Button, fill: u32, cx: &App) -> Button {
        match self {
            Variant::Default => button,
            Variant::Primary => button.primary(),
            Variant::Secondary => button.secondary(),
            Variant::Danger => button.danger(),
            Variant::Ghost => button.ghost(),
            Variant::Selected => button.selected(true),
            Variant::Inverted => button.custom(inverted_button(Hsla::from(rgb(fill)), cx)),
            Variant::ChromeTile => button.custom(chrome_tile_variant(cx)),
        }
    }
}

/// Root view: one fixed-size `Button` on an opaque host fill, nothing else, so the only
/// tab stop in the window is the button and the only fill behind its ring is the host's.
struct RingHost {
    state: Rc<Cell<(u32, Variant)>>,
    disabled: bool,
}

impl Render for RingHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (fill, variant) = self.state.get();
        div()
            .size_full()
            .bg(Hsla::from(rgb(fill)))
            .p(px(PAD))
            .child(
                variant
                    .button(
                        Button::new(BUTTON_ID).w(px(BUTTON_W)).h(px(BUTTON_H)),
                        fill,
                        cx,
                    )
                    .disabled(self.disabled),
            )
    }
}

struct Harness {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    quads: Arc<Mutex<Vec<Quad>>>,
    state: Rc<Cell<(u32, Variant)>>,
}

impl Harness {
    /// A headless window with `theme` installed through the real `apply_theme`.
    fn open(theme: &presets::Theme, disabled: bool) -> Self {
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
            install(cx, theme);
        });
        let state = Rc::new(Cell::new((0xff_ff_ff, Variant::Default)));
        let host_state = state.clone();
        let window = cx
            .open_window(size(px(200.), px(100.)), move |_, cx| {
                cx.new(|_| RingHost {
                    state: host_state,
                    disabled,
                })
            })
            .unwrap();
        let mut harness = Self {
            cx,
            window: window.into(),
            quads,
            state,
        };
        // First frame: registers the button's tab stop.
        harness.draw();
        harness
    }

    /// Draw a frame and return the quads painted by it.
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

    /// Tab traversal, as a keyboard user gets it. Whether something is now focused.
    fn try_focus(&mut self) -> bool {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.focus_next(cx);
                window.focused(cx).is_some()
            })
            .unwrap()
    }

    fn scale_factor(&mut self) -> f32 {
        self.cx
            .update_window(self.window, |_, window, _| window.scale_factor())
            .unwrap()
    }

    /// The border-only quads that are exactly the ring's width on every side. The
    /// button's own border is 1px.
    fn ring_quads(&mut self, quads: &[Quad]) -> Vec<Quad> {
        let width = RING_WIDTH * self.scale_factor();
        quads
            .iter()
            .filter(|quad| {
                let widths = quad.border_widths;
                [widths.top, widths.right, widths.bottom, widths.left]
                    .iter()
                    .all(|w| (w.0 - width).abs() < 0.01)
            })
            .copied()
            .collect()
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

/// Whether `inner` lies within `outer`, to a hundredth of a device pixel.
fn inside(inner: Edges4, outer: Edges4) -> bool {
    const EPS: f32 = 0.01;
    inner[0] >= outer[0] - EPS
        && inner[1] >= outer[1] - EPS
        && inner[2] <= outer[2] + EPS
        && inner[3] <= outer[3] + EPS
}

fn same_colour(a: Hsla, b: Hsla) -> bool {
    [(a.h, b.h), (a.s, b.s), (a.l, b.l), (a.a, b.a)]
        .iter()
        .all(|(x, y)| (x - y).abs() < 1e-4)
}

/// The field wash `apply_theme` installs for every theme, in the order of `themes`, read from the
/// installed theme with the toolkit's own getter (`input_background()`). One headless app is
/// reused and each theme is installed through the real `apply_theme`, so nothing about the wash
/// is reconstructed here.
fn installed_washes(themes: &[presets::Theme]) -> Vec<Hsla> {
    let mut host = Harness::open(&themes[0], false);
    host.cx.update(|cx| {
        cx.set_global(presets::Themes(themes.to_vec()));
        themes
            .iter()
            .map(|theme| {
                install(cx, theme);
                cx.theme().input_background()
            })
            .collect()
    })
}

/// The themes the painted witness draws: stock dark and light, the three whose ring is
/// tightest against the union, and the three tightest among those whose ink moved off
/// the accent seed. Chosen from the real derivation, deterministically. `washes[i]` is the
/// field wash `apply_theme` installed for `themes[i]`, read by [`installed_washes`].
fn witness_themes(themes: &[presets::Theme], washes: &[Hsla]) -> Vec<presets::Theme> {
    assert_eq!(themes.len(), washes.len(), "one installed wash per theme");
    let mut scored: Vec<(f32, bool, usize)> = Vec::new();
    for (index, theme) in themes.iter().enumerate() {
        let resolved = theme.resolve();
        let ink = theme.focus_ring_ink_from(&resolved, washes[index]);
        let margin = theme
            .focus_ring_field_fills(&resolved, washes[index])
            .iter()
            .map(|&fill| contrast(ink, fill))
            .fold(f32::MAX, f32::min);
        scored.push((margin, ink != resolved.m.accent, index));
    }
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.2.cmp(&b.2)));
    let mut picks: Vec<usize> = themes
        .iter()
        .enumerate()
        .filter(|(_, theme)| theme.id == "light" || theme.id == "dark")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(picks.len(), 2, "the stock dark and light built-ins");
    picks.extend(scored.iter().take(3).map(|s| s.2));
    let moved: Vec<usize> = scored.iter().filter(|s| s.1).take(3).map(|s| s.2).collect();
    assert!(
        !moved.is_empty(),
        "no theme moved its ink: nothing to witness"
    );
    picks.extend(moved);
    picks.sort_unstable();
    picks.dedup();
    picks
        .into_iter()
        .map(|index| themes[index].clone())
        .collect()
}

/// A real focused toolkit `Button` of every variant, on every fill the ring can float
/// over, in the stock themes and in the tightest and moved-ink ones: the ring quad is
/// the installed ink, opaque, exactly 2px off the control all the way round, nothing but
/// the host fill is painted in that gap, and the painted ring against the painted host
/// fill is at least 3:1.
#[test]
fn the_painted_ring_is_the_installed_ink_two_pixels_out_on_every_fill_and_variant() {
    crate::core::config::pin_test_config_dir();
    let themes = all_themes();
    let washes = installed_washes(&themes);
    let picks = witness_themes(&themes, &washes);
    let mut misses = Vec::new();
    let mut frames = 0usize;
    for theme in &picks {
        let mut harness = Harness::open(theme, false);
        let scale = harness.scale_factor();
        let ink: Hsla = harness
            .cx
            .update(|cx| cx.theme().focus_ring.expect("a ring is installed"));
        let fills = harness.cx.update(|cx| installed_fills(cx, theme));
        harness.state.set((fills[0].1, Variant::Default));
        harness.draw();
        assert!(
            harness.try_focus(),
            "{}: tab traversal did not focus the button",
            theme.id
        );

        let control = [
            PAD * scale,
            PAD * scale,
            (PAD + BUTTON_W) * scale,
            (PAD + BUTTON_H) * scale,
        ];
        let ring_outer = outset(control, (GAP + RING_WIDTH) * scale);
        let window_box = [0., 0., 200. * scale, 100. * scale];
        for (fill_name, fill) in &fills {
            for variant in VARIANTS {
                frames += 1;
                let what = format!("{} / {fill_name} {fill:06x} / {variant:?}", theme.id);
                harness.state.set((*fill, variant));
                let quads = harness.draw();
                let rings = harness.ring_quads(&quads);
                if rings.len() != 1 {
                    misses.push(format!("{what}: {} ring quads, expected 1", rings.len()));
                    continue;
                }
                let ring = rings[0];
                if !same_colour(ring.border_color, ink) {
                    misses.push(format!(
                        "{what}: ring painted {:?}, installed {ink:?}",
                        ring.border_color
                    ));
                }
                if (ring.border_color.a - 1.0).abs() > 1e-6 {
                    misses.push(format!("{what}: ring alpha {}", ring.border_color.a));
                }
                let got = edges(&ring);
                if got
                    .iter()
                    .zip(ring_outer.iter())
                    .any(|(g, w)| (g - w).abs() > 0.01)
                {
                    misses.push(format!(
                        "{what}: ring edges {got:?}, expected the control outset {} px: {ring_outer:?}",
                        (GAP + RING_WIDTH) * scale
                    ));
                }
                // The host fill that was actually painted behind the ring.
                let want_fill = format!("{:?}", Background::from(Hsla::from(rgb(*fill))));
                let hosts: Vec<&Quad> = quads
                    .iter()
                    .filter(|quad| {
                        inside(window_box, edges(quad))
                            && format!("{:?}", quad.background) == want_fill
                    })
                    .collect();
                if hosts.len() != 1 {
                    misses.push(format!(
                        "{what}: {} host fill quads, expected 1",
                        hosts.len()
                    ));
                }
                // Nothing but the host is painted in the gap: every other quad is the
                // ring, or lies inside the control.
                for quad in &quads {
                    let e = edges(quad);
                    let is_ring = rings
                        .iter()
                        .any(|r| format!("{r:?}") == format!("{quad:?}"));
                    let is_host = inside(window_box, e);
                    if !is_ring && !is_host && !inside(e, control) {
                        misses.push(format!(
                            "{what}: a quad {e:?} is painted outside the control and the ring"
                        ));
                    }
                }
                let ratio = contrast(hsla24(ring.border_color), *fill);
                if ratio < FLOOR {
                    misses.push(format!(
                        "{what}: painted ring is only {ratio:.2}:1 on the host fill"
                    ));
                }
            }
        }
    }
    assert!(
        picks.len() >= 3,
        "only {} themes were witnessed",
        picks.len()
    );
    println!(
        "painted ring witnessed in {} themes over {frames} frames",
        picks.len()
    );
    assert_no_misses("painted focus ring", misses);
}

/// A disabled button is not a tab stop and draws no ring; its enabled twin in the same
/// theme does, so the check can fail.
#[test]
fn a_disabled_button_takes_no_focus_and_draws_no_ring() {
    crate::core::config::pin_test_config_dir();
    for id in ["light", "dark"] {
        let theme = presets::builtins()
            .into_iter()
            .find(|theme| theme.id == id)
            .expect("a stock theme");
        let mut enabled = Harness::open(&theme, false);
        assert!(
            enabled.try_focus(),
            "{id}: the enabled twin did not take focus"
        );
        let quads = enabled.draw();
        assert_eq!(
            enabled.ring_quads(&quads).len(),
            1,
            "{id}: the enabled twin has no ring"
        );

        let mut disabled = Harness::open(&theme, true);
        assert!(!disabled.try_focus(), "{id}: a disabled button took focus");
        let quads = disabled.draw();
        assert!(
            disabled.ring_quads(&quads).is_empty(),
            "{id}: a disabled button drew a ring"
        );
    }
}
