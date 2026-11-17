//! Settings keyboard controls (package S), tests first. Drives the real
//! settings render, real Tab / Enter / Space / arrow keys and the real
//! callbacks.
//!
//! Four independent witnesses; no test lets one stand in for another:
//! * FOCUS: the innermost `tty59-stop` marker (`kit::mark`, cfg(test), value =
//!   the control's own element id) on `window.context_stack()` of the LIVE
//!   focused path. It makes no handle, stop or ring.
//! * STATE: what the real callback did to `Config` / `SettingsState` / the
//!   fixture daemon.
//! * ACTIVATION: a real key down then key up (`press`), the key-up being what
//!   fires a focused element's click listeners.
//! * PAINT: ring bounds (`{id}-ring`, the tag the ring gets under cfg(test)),
//!   only in the R10 tests.
//!
//! Positive controls (`p*`) pass on today's tree: they show the selectors, the
//! callbacks, real Tab and the marker reader are the real ones, so a RED
//! result is about reach, not about the harness.
//!
//! Deferred, post-implementation reviewer tests (they need the Stop API that
//! does not exist yet): the same builder rendered bare and through `stop` in
//! six parents with identical bounds, and a slider whose `on_set` / `on_release`
//! closures are counted once per key press.

use std::io::ErrorKind;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::kit::test_support::harness;
use super::*;
use gpui::{KeyUpEvent, Keystroke, Modifiers, TestAppContext, VisualTestContext, size};

const MAX_TABS: usize = 160;

// ------------------------------------------------------------------ helpers

fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..2 {
        vcx.run_until_parked();
        vcx.update(|w, cx| {
            let _ = w.draw(cx);
        });
    }
}

/// A key as the keyboard delivers it: down, then up.
fn press(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    vcx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).expect("a key name"),
    });
    settle(vcx);
}

fn tab(vcx: &mut VisualTestContext) {
    vcx.simulate_keystrokes("tab");
    settle(vcx);
}

fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    let at = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    vcx.simulate_click(at.center(), Modifiers::none());
    settle(vcx);
}

fn drawn(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    vcx.debug_bounds(selector).is_some()
}

/// Selectors built from ids at run time (`ssh-host-{n}`) still have to be
/// `&'static str` for `debug_bounds`; a test leaks the few it needs.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// FOCUS witness: the innermost marker on the focused path, if any.
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

/// Real focus on the page's search Input: where a person's Tab starts.
fn focus_search(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> gpui::FocusHandle {
    use gpui::Focusable as _;
    let handle = app.update_in(vcx, |app, window, cx| {
        let search = app.active_settings().expect("settings open").search.clone();
        search.update(cx, |s, cx| s.focus(window, cx));
        search.read(cx).focus_handle(cx)
    });
    settle(vcx);
    assert!(
        vcx.update(|w, _| handle.is_focused(w)),
        "setup: the search Input holds focus"
    );
    handle
}

/// Real Tabs from where focus is until `pred` accepts the FOCUS marker.
/// Bounded; gives up on a wrap onto a stop already visited or on no focus.
fn tab_until(vcx: &mut VisualTestContext, pred: impl Fn(&str) -> bool) -> Option<String> {
    let mut seen = vec![focused(vcx)];
    for _ in 0..MAX_TABS {
        if let Some(m) = marker(vcx).filter(|m| pred(m)) {
            return Some(m);
        }
        tab(vcx);
        let now = focused(vcx);
        if now.is_none() || seen.contains(&now) {
            return marker(vcx).filter(|m| pred(m));
        }
        seen.push(now);
    }
    None
}

fn tab_to(vcx: &mut VisualTestContext, id: &str) -> bool {
    tab_until(vcx, |m| m == id).is_some()
}

/// Every marker met on one forward lap of real Tabs from the search Input.
fn lap(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Vec<String> {
    let start = focus_search(app, vcx);
    let mut seen = vec![Some(start)];
    let mut out = Vec::new();
    for _ in 0..MAX_TABS {
        tab(vcx);
        let now = focused(vcx);
        if now.is_none() || seen.contains(&now) {
            break;
        }
        seen.push(now);
        if let Some(m) = marker(vcx) {
            out.push(m);
        }
    }
    out
}

fn open(
    cx: &mut TestAppContext,
    section: SettingsSection,
) -> (Entity<Tty59App>, VisualTestContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(section, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    settle(&mut vcx);
    (app, vcx)
}

fn cfg<T>(vcx: &mut VisualTestContext, f: impl FnOnce(&Config) -> T) -> T {
    vcx.update(|_, cx| f(cx.global::<Config>()))
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

fn settings_open(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
    app.update_in(vcx, |app, _, _| app.active_settings().is_some())
}

fn same_box(a: gpui::Bounds<gpui::Pixels>, b: gpui::Bounds<gpui::Pixels>) -> bool {
    let near = |x: gpui::Pixels, y: gpui::Pixels| (x - y).abs() <= px(0.5);
    near(a.origin.x, b.origin.x)
        && near(a.origin.y, b.origin.y)
        && near(a.size.width, b.size.width)
        && near(a.size.height, b.size.height)
}

fn grown(b: gpui::Bounds<gpui::Pixels>, by: gpui::Pixels) -> gpui::Bounds<gpui::Pixels> {
    gpui::Bounds {
        origin: gpui::point(b.origin.x - by, b.origin.y - by),
        size: gpui::size(b.size.width + by * 2.0, b.size.height + by * 2.0),
    }
}

// ------------------------------------------------- positive controls (pass today)

/// P1: the selector and the callback are the real ones: a pointer click on the
/// SAME mounted control flips the SAME config value R1 reads.
#[gpui::test]
fn p1_a_click_on_the_mouse_hide_switch_flips_its_config(cx: &mut TestAppContext) {
    let (_app, mut vcx) = open(cx, SettingsSection::KeyboardMouse);
    let before = cfg(&mut vcx, |c| c.mouse_hide_while_typing);
    click(&mut vcx, "term-mouse-hide");
    assert_eq!(
        cfg(&mut vcx, |c| c.mouse_hide_while_typing),
        !before,
        "the click is the activation R1 expects from the keyboard"
    );
}

/// P2: a real Tab moves real focus off an existing Input onto an existing stop
/// (the Keybindings page already has tab stops). A RED reach result is
/// therefore about the settings controls, not about Input swallowing Tab. The
/// cells carry no marker, so a null marker here means "unmarked", not "no
/// focus".
#[gpui::test]
fn p2_tab_leaves_an_input_for_an_existing_stop(cx: &mut TestAppContext) {
    use gpui::Focusable as _;
    let (app, mut vcx) = open(cx, SettingsSection::Keybindings);
    let field = app.update_in(&mut vcx, |app, window, cx| {
        let f = app.active_settings().unwrap().shortcut_search.clone();
        f.update(cx, |f, cx| f.focus(window, cx));
        f.read(cx).focus_handle(cx)
    });
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| field.is_focused(w)),
        "setup: field focused"
    );
    tab(&mut vcx);
    let now = focused(&mut vcx);
    assert!(
        now.is_some() && now.as_ref() != Some(&field),
        "a real Tab moved focus off the shortcut filter onto another stop"
    );
}

/// P3: the marker reader reads a marker off a really focused, really tabbed
/// control, in a window of its own, with no settings code involved.
#[gpui::test]
fn p3_the_marker_is_read_off_a_really_focused_stop(cx: &mut TestAppContext) {
    use gpui::{Context, IntoElement, ParentElement as _, Render, Window, div, prelude::*};
    struct Fx {
        a: gpui::FocusHandle,
        b: gpui::FocusHandle,
    }
    impl Render for Fx {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(kit::mark(
                    div().id("fx-a").track_focus(&self.a.clone().tab_stop(true)),
                    "fx-a",
                ))
                .child(kit::mark(
                    div().id("fx-b").track_focus(&self.b.clone().tab_stop(true)),
                    "fx-b",
                ))
        }
    }
    cx.update(gpui_component::init);
    let window = cx.add_window(|_, cx| Fx {
        a: cx.focus_handle(),
        b: cx.focus_handle(),
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    settle(&mut vcx);
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert_eq!(marker(&mut vcx).as_deref(), Some("fx-a"));
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert_eq!(marker(&mut vcx).as_deref(), Some("fx-b"));
}

/// P4: a click on Add host opens the new-host form: the effect R2 expects.
#[gpui::test]
fn p4_a_click_on_add_host_opens_the_form(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Ssh);
    assert!(state(&app, &mut vcx, |s| s.ssh_form.is_none()), "setup");
    click(&mut vcx, "ssh-add-host");
    assert!(state(&app, &mut vcx, |s| s.ssh_form.is_some()));
}

fn one_host(
    cx: &mut TestAppContext,
    group: Option<&str>,
) -> (Entity<Tty59App>, VisualTestContext, uuid::Uuid) {
    let (app, mut vcx) = harness(cx);
    let id = app.update_in(&mut vcx, |app, window, cx| {
        let mut p = crate::core::ssh_profile::SshProfile::new("alpha");
        p.host = "alpha.example".into();
        p.group = group.map(str::to_string);
        let id = p.id;
        cx.global_mut::<Config>().ssh_profiles = vec![p];
        app.open_settings_section(SettingsSection::Ssh, window, cx);
        if group.is_some() {
            app.active_settings_mut().unwrap().ssh_show_all = true;
        }
        id
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    settle(&mut vcx);
    (app, vcx, id)
}

/// P5: a click on a host row head toggles `ssh_open`; on a group head, the
/// group's collapsed flag.
#[gpui::test]
fn p5_clicks_on_a_host_head_and_a_group_head_toggle_them(cx: &mut TestAppContext) {
    let (app, mut vcx, id) = one_host(cx, None);
    let head = leak(format!("ssh-host-{}", id.as_u128()));
    click(&mut vcx, head);
    assert_eq!(state(&app, &mut vcx, |s| s.ssh_open), Some(id));
    click(&mut vcx, head);
    assert_eq!(state(&app, &mut vcx, |s| s.ssh_open), None);

    let (app, mut vcx, _id) = one_host(cx, Some("grp"));
    click(&mut vcx, "ssh-group-grp");
    assert!(state(&app, &mut vcx, |s| s
        .ssh_collapsed_groups
        .contains("grp")));
    click(&mut vcx, "ssh-group-grp");
    assert!(state(&app, &mut vcx, |s| !s
        .ssh_collapsed_groups
        .contains("grp")));
}

// ------------------------------------------------------------------ R1 Switch

#[gpui::test]
fn r1_tab_reaches_the_mouse_hide_switch_and_space_and_enter_flip_it(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::KeyboardMouse);
    let before = cfg(&mut vcx, |c| c.mouse_hide_while_typing);
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "term-mouse-hide"),
        "Tab never reached term-mouse-hide"
    );
    press(&mut vcx, "space");
    assert_eq!(
        cfg(&mut vcx, |c| c.mouse_hide_while_typing),
        !before,
        "Space"
    );
    press(&mut vcx, "enter");
    assert_eq!(
        cfg(&mut vcx, |c| c.mouse_hide_while_typing),
        before,
        "Enter"
    );
}

#[gpui::test]
fn r1b_the_decorative_modified_switch_is_never_a_stop(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::KeyboardMouse);
    let stops = lap(&app, &mut vcx);
    assert!(
        stops.iter().any(|m| m == "settings-modified-filter"),
        "Tab never reached the modified-only row settings-modified-filter ({stops:?})"
    );
    assert!(
        !stops.iter().any(|m| m == "settings-modified-switch"),
        "the handler-less switch inside the row is not a stop: {stops:?}"
    );
}

// ------------------------------------------------------------------ R2 Btn

#[gpui::test]
fn r2_enter_on_add_host_does_what_its_click_does_and_dimmed_save_is_a_stop(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx) = open(cx, SettingsSection::Ssh);
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "ssh-add-host"),
        "Tab never reached ssh-add-host"
    );
    press(&mut vcx, "enter");
    assert!(
        state(&app, &mut vcx, |s| s.ssh_form.is_some()),
        "Enter opens the form, as P4's click"
    );
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "ssh-form-save"),
        "a dimmed-but-clickable Btn is a stop"
    );
}

// ------------------------------------------------------------------ R3 nav

#[gpui::test]
fn r3_tab_from_search_reaches_nav_and_enter_changes_section(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::General);
    focus_search(&app, &mut vcx);
    let here = state(&app, &mut vcx, |s| s.section);
    assert!(
        tab_until(&mut vcx, |m| m.starts_with("settings-nav-")).is_some(),
        "Tab from the search never reached a nav item"
    );
    // The second nav stop is never the current section's.
    tab(&mut vcx);
    assert!(marker(&mut vcx).is_some_and(|m| m.starts_with("settings-nav-")));
    press(&mut vcx, "enter");
    assert!(
        state(&app, &mut vcx, |s| s.section) != here,
        "Enter navigated"
    );
}

#[gpui::test]
fn r3b_enter_on_the_modified_only_row_toggles_the_filter(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::General);
    focus_search(&app, &mut vcx);
    let before = state(&app, &mut vcx, |s| s.modified_only);
    assert!(tab_to(&mut vcx, "settings-modified-filter"));
    press(&mut vcx, "enter");
    assert_eq!(state(&app, &mut vcx, |s| s.modified_only), !before);
}

// ------------------------------------------------------------------ R4 per-row Reset

#[gpui::test]
fn r4_enter_on_a_modified_rows_reset_restores_the_default(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::KeyboardMouse);
    let default = Config::default().mouse_hide_while_typing;
    // The click path makes the row modified, so its Reset is drawn.
    click(&mut vcx, "term-mouse-hide");
    assert_ne!(
        cfg(&mut vcx, |c| c.mouse_hide_while_typing),
        default,
        "setup"
    );
    focus_search(&app, &mut vcx);
    assert!(
        tab_until(&mut vcx, |m| m.starts_with("reset-setting-")).is_some(),
        "Tab never reached a per-row Reset"
    );
    press(&mut vcx, "enter");
    assert_eq!(cfg(&mut vcx, |c| c.mouse_hide_while_typing), default);
}

// ------------------------------------------------------------------ R5 close / back

#[gpui::test]
fn r5_the_lap_ends_on_close_and_enter_closes_settings(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::General);
    let stops = lap(&app, &mut vcx);
    assert_eq!(
        stops.last().map(String::as_str),
        Some("settings-close"),
        "{stops:?}"
    );
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, "settings-close"));
    press(&mut vcx, "enter");
    assert!(!settings_open(&app, &mut vcx), "Enter closed settings");
}

#[gpui::test]
fn r5b_enter_on_back_returns_to_keyboard_and_mouse(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Keybindings);
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, "settings-back"));
    press(&mut vcx, "enter");
    assert!(
        state(&app, &mut vcx, |s| s.section) == SettingsSection::KeyboardMouse,
        "Enter on Back went to Keyboard & Mouse"
    );
}

// ------------------------------------------------------------------ R6 dropdown

#[gpui::test]
fn r6_the_trigger_opens_walks_and_picks_without_the_keyup_picking(cx: &mut TestAppContext) {
    use crate::core::config::CursorStyle;
    let (app, mut vcx) = open(cx, SettingsSection::Appearance);
    let before = cfg(&mut vcx, |c| c.cursor_style);
    assert_eq!(
        before,
        CursorStyle::Block,
        "setup: Block is first, Down lands on Bar"
    );
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, "cursor-style-trigger"));
    press(&mut vcx, "enter");
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu.clone()).as_deref(),
        Some("cursor-style"),
        "Enter opens, and the same key-up did not toggle it shut"
    );
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("cursor-style-trigger"),
        "focus stays on the trigger"
    );
    assert_eq!(
        cfg(&mut vcx, |c| c.cursor_style),
        before,
        "the key-up did not pick the highlighted row"
    );
    press(&mut vcx, "down");
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu_hi),
        1,
        "Down moves the highlight"
    );
    press(&mut vcx, "enter");
    assert_eq!(
        cfg(&mut vcx, |c| c.cursor_style),
        CursorStyle::Bar,
        "Enter picks the highlighted row"
    );
    assert!(state(&app, &mut vcx, |s| s.menu.is_none()), "and closes");
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("cursor-style-trigger"),
        "focus is back on the trigger"
    );
}

#[gpui::test]
fn r6b_escape_closes_without_picking_and_down_opens_a_closed_trigger(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Appearance);
    let before = cfg(&mut vcx, |c| c.cursor_style);
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, "cursor-style-trigger"));
    press(&mut vcx, "down");
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_some()),
        "Down opens a closed trigger"
    );
    press(&mut vcx, "down");
    press(&mut vcx, "escape");
    assert!(state(&app, &mut vcx, |s| s.menu.is_none()), "Escape closes");
    assert_eq!(
        cfg(&mut vcx, |c| c.cursor_style),
        before,
        "and picked nothing"
    );
    assert!(settings_open(&app, &mut vcx), "Escape peeled the menu only");
}

// ------------------------------------------------------------------ R7 hosts

#[gpui::test]
fn r7_enter_on_a_host_head_toggles_it_and_is_inert_while_it_is_edited(cx: &mut TestAppContext) {
    let (app, mut vcx, id) = one_host(cx, None);
    let head = format!("ssh-host-{}", id.as_u128());
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, &head), "Tab never reached the host head");
    press(&mut vcx, "enter");
    assert_eq!(
        state(&app, &mut vcx, |s| s.ssh_open),
        Some(id),
        "Enter opens"
    );
    press(&mut vcx, "enter");
    assert_eq!(state(&app, &mut vcx, |s| s.ssh_open), None, "Enter closes");
    press(&mut vcx, "enter");
    // The opened host's own Edit button, then back to the head while edited.
    assert!(
        tab_to(&mut vcx, "ssh-host-edit"),
        "Edit is a stop in the details"
    );
    press(&mut vcx, "enter");
    assert!(state(&app, &mut vcx, |s| s.ssh_form.is_some()), "editing");
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, &head));
    press(&mut vcx, "enter");
    assert_eq!(
        state(&app, &mut vcx, |s| s.ssh_open),
        Some(id),
        "Enter on a host being edited does nothing, as its click does not"
    );
}

#[gpui::test]
fn r7b_enter_on_a_group_head_collapses_and_expands_it(cx: &mut TestAppContext) {
    let (app, mut vcx, _id) = one_host(cx, Some("grp"));
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "ssh-group-grp"),
        "Tab never reached the group head"
    );
    press(&mut vcx, "enter");
    assert!(state(&app, &mut vcx, |s| s
        .ssh_collapsed_groups
        .contains("grp")));
    press(&mut vcx, "enter");
    assert!(state(&app, &mut vcx, |s| !s
        .ssh_collapsed_groups
        .contains("grp")));
}

#[gpui::test]
fn r7c_an_opened_hosts_connect_button_is_a_stop_but_is_not_pressed(cx: &mut TestAppContext) {
    // Pressing it would start a real ssh connect, so this reaches it only.
    let (app, mut vcx, id) = one_host(cx, None);
    click(&mut vcx, leak(format!("ssh-host-{}", id.as_u128())));
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, "ssh-host-connect"));
}

// ------------------------------------------------------------------ R8 stepper

#[gpui::test]
fn r8_the_stepper_is_one_stop_and_arrows_step_it(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Appearance);
    let start = cfg(&mut vcx, |c| c.font_size);
    let stops = lap(&app, &mut vcx);
    assert_eq!(
        stops.iter().filter(|m| m.starts_with("font-size")).count(),
        1,
        "one stop for the whole control: {stops:?}"
    );
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "font-size"),
        "Tab never reached the stepper"
    );
    press(&mut vcx, "right");
    assert!(cfg(&mut vcx, |c| c.font_size) > start, "Right increments");
    press(&mut vcx, "left");
    press(&mut vcx, "left");
    assert!(cfg(&mut vcx, |c| c.font_size) < start, "Left decrements");
}

#[gpui::test]
fn r8b_a_disabled_end_ignores_its_key(cx: &mut TestAppContext) {
    use crate::core::config::FONT_SIZE_MAX;
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        cx.global_mut::<Config>().font_size = FONT_SIZE_MAX;
        app.open_settings_section(SettingsSection::Appearance, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    settle(&mut vcx);
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, "font-size"));
    press(&mut vcx, "right");
    press(&mut vcx, "up");
    assert_eq!(
        cfg(&mut vcx, |c| c.font_size),
        FONT_SIZE_MAX,
        "at the maximum"
    );
}

// ------------------------------------------------------------------ R9 sliders

/// Persistence visibility, not a write count: after each key press the
/// settings page's saved snapshot already equals the live value, as after a
/// drag release. A once-per-press count is a separate, post-implementation
/// reviewer control on a slider built here with counting closures.
#[gpui::test]
fn r9_opacity_slider_steps_four_percent_per_arrow_and_is_saved_each_press(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Appearance);
    let live = |vcx: &mut VisualTestContext| cfg(vcx, |c| c.window_opacity);
    let saved = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        state(app, vcx, |s| s.saved_config.window_opacity)
    };
    let start = vcx.update(|_, cx| Tty59App::effective_window_opacity(cx));
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "window-opacity"),
        "Tab never reached the slider"
    );
    let mut want = start;
    for _ in 0..3 {
        press(&mut vcx, "left");
        want = (want - 0.04).max(0.2);
        assert_eq!(
            saved(&app, &mut vcx),
            live(&mut vcx),
            "the saved snapshot follows every press"
        );
        let now = live(&mut vcx).expect("a press sets the opacity");
        assert!(
            (now - want).abs() < 0.011,
            "{now} vs {want}: 0.05 of the 0.2..1.0 range"
        );
    }
    press(&mut vcx, "home");
    assert_eq!(live(&mut vcx), Some(0.2), "Home = minimum");
    press(&mut vcx, "end");
    assert_eq!(live(&mut vcx), Some(1.0), "End = maximum");
}

#[gpui::test]
fn r9b_scroll_slider_steps_a_whole_quarter_and_never_stalls(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Terminal);
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, "term-scroll-speed"));
    let mut seen = vec![cfg(&mut vcx, |c| c.mouse_scroll_multiplier)];
    for _ in 0..3 {
        press(&mut vcx, "right");
        seen.push(cfg(&mut vcx, |c| c.mouse_scroll_multiplier));
    }
    assert!(
        seen.windows(2).all(|w| w[1] > w[0]),
        "each Right moves it: {seen:?}"
    );
    assert!(
        seen.iter()
            .all(|v| ((v / 0.25).round() * 0.25 - v).abs() < 1e-3),
        "quantised to 0.25: {seen:?}"
    );
    press(&mut vcx, "home");
    assert!((cfg(&mut vcx, |c| c.mouse_scroll_multiplier) - 0.5).abs() < 1e-3);
    press(&mut vcx, "end");
    assert!((cfg(&mut vcx, |c| c.mouse_scroll_multiplier) - 5.0).abs() < 1e-3);
}

// ------------------------------------------------------------------ R10 ring (PAINT only)
//
// FOCUS is read from the marker, PAINT from the ring's tagged bounds. The ring
// is tagged `{id}-ring` under cfg(test) by whatever draws it.
//
// Today these fail at their FOCUS assertion (focus never reaches the control),
// so they show no paint result: the PAINT assertions are only reached once the
// stops exist, and paint mutants (no ring, wrong gap, ring on pointer focus)
// are required then, not claimed here.

fn ring_gap(vcx: &mut VisualTestContext) -> gpui::Pixels {
    vcx.update(|_, cx| {
        use gpui_component::ActiveTheme as _;
        cx.theme().focus_ring_offset()
    })
}

#[gpui::test]
fn r10_a_keyboard_focused_stop_draws_the_ring_around_the_control(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::KeyboardMouse);
    assert!(
        !drawn(&mut vcx, "term-mouse-hide-ring"),
        "no ring before anything is focused"
    );
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "term-mouse-hide"),
        "FOCUS: reach comes first"
    );
    let control = vcx
        .debug_bounds("term-mouse-hide")
        .expect("the switch is drawn");
    let ring = vcx
        .debug_bounds("term-mouse-hide-ring")
        .expect("PAINT: a keyboard-focused stop draws no ring");
    let by = ring_gap(&mut vcx) + px(1.5);
    assert!(
        same_box(ring, grown(control, by)),
        "the ring {ring:?} is the control {control:?} grown by the gap and the stroke"
    );
    focus_search(&app, &mut vcx);
    assert!(
        !drawn(&mut vcx, "term-mouse-hide-ring"),
        "the ring goes when focus does, and an Input draws no stop ring"
    );
}

#[gpui::test]
fn r10b_a_pointer_click_focuses_the_stop_but_draws_no_ring(cx: &mut TestAppContext) {
    let (_app, mut vcx) = open(cx, SettingsSection::KeyboardMouse);
    click(&mut vcx, "term-mouse-hide");
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("term-mouse-hide"),
        "FOCUS: a pointer click on the switch leaves the switch itself focused"
    );
    assert!(
        !drawn(&mut vcx, "term-mouse-hide-ring"),
        "PAINT: pointer-modality focus is not ringed (keyboard_only)"
    );
}

// ------------------------------------------------------------------ R11 layout baselines (actual pages)
//
// Today's sizes of real controls, and "walking focus moves nothing". They pass
// now and must keep passing once the stops and rings exist.

fn bounds_of(
    vcx: &mut VisualTestContext,
    selectors: &[&'static str],
) -> Vec<(&'static str, gpui::Bounds<gpui::Pixels>)> {
    selectors
        .iter()
        .map(|s| {
            (
                *s,
                vcx.debug_bounds(s)
                    .unwrap_or_else(|| panic!("{s} is not drawn")),
            )
        })
        .collect()
}

#[gpui::test]
fn r11_the_switch_keeps_its_size(cx: &mut TestAppContext) {
    let (_app, mut vcx) = open(cx, SettingsSection::KeyboardMouse);
    let sw = vcx.debug_bounds("term-mouse-hide").unwrap();
    assert_eq!(
        (sw.size.width, sw.size.height),
        (px(30.), px(18.)),
        "Switch"
    );
}

#[gpui::test]
fn r11a_the_secondary_button_keeps_its_height(cx: &mut TestAppContext) {
    let (_app, mut vcx) = open(cx, SettingsSection::Ssh);
    let add = vcx.debug_bounds("ssh-add-host").unwrap();
    assert_eq!(add.size.height, px(26.), "Secondary Btn height");
}

#[gpui::test]
fn r11a2_the_slider_and_the_stepper_keep_their_sizes(cx: &mut TestAppContext) {
    let (_app, mut vcx) = open(cx, SettingsSection::Appearance);
    let slider = vcx.debug_bounds("window-opacity").unwrap();
    assert_eq!(
        (slider.size.width, slider.size.height),
        (px(148.), px(18.)),
        "slider track"
    );
    let stepper = vcx.debug_bounds("font-size").unwrap();
    assert_eq!(stepper.size.height, px(26.), "stepper");
}

#[gpui::test]
fn r11b_walking_focus_around_a_page_moves_no_control(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Appearance);
    let watched = [
        "window-opacity",
        "font-size",
        "cursor-style-trigger",
        "settings-close",
    ];
    let before = bounds_of(&mut vcx, &watched);
    let _ = lap(&app, &mut vcx);
    assert_eq!(
        bounds_of(&mut vcx, &watched),
        before,
        "a Tab lap (a ring drawn on each stop in turn) shifts nothing"
    );
}

// ------------------------------------------------------------------ R12 guards

#[gpui::test]
fn r12_while_recording_tab_enter_and_space_reach_no_stop(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Keybindings);
    vcx.simulate_resize(size(px(1100.), px(4000.)));
    settle(&mut vcx);
    let recording = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        state(app, vcx, |s| {
            s.recording
                .as_ref()
                .map(|r| (r.action.clone(), r.chords.clone()))
        })
    };
    // Each key gets its own recording: the recorder takes a key as a chord
    // and a later key would otherwise meet a recorder that already ended. A
    // real click on New Tab's cell arms it and gives that cell the focus.
    for key in ["tab", "enter", "space"] {
        click(&mut vcx, "kb-NewTab");
        assert_eq!(
            recording(&app, &mut vcx).map(|(action, _)| action),
            Some("NewTab".to_string()),
            "setup ({key}): the click arms the recorder"
        );
        let cell = focused(&mut vcx);
        assert!(cell.is_some(), "setup ({key}): the cell holds focus");
        let section = state(&app, &mut vcx, |s| s.section);
        let before = cfg(&mut vcx, |c| c.mouse_hide_while_typing);

        // As the F20 test the_key_up_of_a_recorded_enter_does_not_stop_the_recording:
        // the key goes in as exactly one chord named after it, and its key-up
        // neither stops the recording nor loses or adds a chord.
        let owned = Some(("NewTab".to_string(), vec![key.to_string()]));
        vcx.simulate_keystrokes(key);
        settle(&mut vcx);
        assert_eq!(
            recording(&app, &mut vcx),
            owned,
            "{key} went in as the one chord"
        );
        vcx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("a key name"),
        });
        settle(&mut vcx);
        assert_eq!(
            recording(&app, &mut vcx),
            owned,
            "{key}: its key-up did not stop the recording or change the chords"
        );
        assert!(
            focused(&mut vcx) == cell,
            "{key}: focus did not move to another stop"
        );
        assert_eq!(marker(&mut vcx), None, "{key} reached a marked stop");
        assert!(settings_open(&app, &mut vcx), "{key}: settings stayed open");
        assert!(
            state(&app, &mut vcx, |s| s.section) == section,
            "{key}: no navigation stop fired"
        );
        assert_eq!(
            cfg(&mut vcx, |c| c.mouse_hide_while_typing),
            before,
            "{key}: no switch fired"
        );

        // Escape is the recorder's own way out; the next key re-arms.
        press(&mut vcx, "escape");
        assert_eq!(
            recording(&app, &mut vcx),
            None,
            "{key}: Escape ends the recording"
        );
        assert!(
            settings_open(&app, &mut vcx),
            "{key}: Escape went to the recorder, not the page"
        );
    }
}

#[gpui::test]
fn r12b_text_keys_in_the_search_input_edit_it_and_fire_no_stop(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::General);
    let search = focus_search(&app, &mut vcx);
    vcx.simulate_input("a b");
    settle(&mut vcx);
    let value = app.update_in(&mut vcx, |app, _, cx| {
        app.active_settings()
            .unwrap()
            .search
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(value, "a b", "Space and letters edit the text");
    assert!(
        vcx.update(|w, _| search.is_focused(w)),
        "focus stayed in the Input"
    );
    assert_eq!(marker(&mut vcx), None, "no stop took the focus");
}

#[gpui::test]
fn r12c_escape_closes_settings_from_a_focused_stop(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::General);
    focus_search(&app, &mut vcx);
    assert!(tab_to(&mut vcx, "settings-modified-filter"));
    press(&mut vcx, "escape");
    assert!(
        !settings_open(&app, &mut vcx),
        "a focused stop does not swallow Escape (menu, conflict and search are not open)"
    );
}

#[gpui::test]
fn r12d_search_results_arrows_and_enter_still_walk_the_results(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::General);
    focus_search(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        let search = app.active_settings().unwrap().search.clone();
        search.update(cx, |i, cx| i.set_value("mouse".to_string(), window, cx));
        app.autoselect_settings_search(cx);
    });
    settle(&mut vcx);
    let here = state(&app, &mut vcx, |s| s.section);
    press(&mut vcx, "down");
    assert_eq!(
        state(&app, &mut vcx, |s| s.search_selection),
        1,
        "Down moves the result selection"
    );
    press(&mut vcx, "enter");
    assert!(
        state(&app, &mut vcx, |s| s.section) != here,
        "Enter opens the selected result"
    );
}

// ------------------------------------------------------------------ V0-V3 reveal (narrow window)
//
// A stop below the scroll viewport must be brought into view by a real Tab.
// Four independent witnesses, none standing in for another:
// * V0 setup control (passes today): in a window too short for the page the
//   target starts OFF-screen, proven by its laid-out bounds against the
//   content scroll's own viewport and a zero offset, not assumed from a size.
// * V1 FOCUS then VISIBLE: real Tab until the marker names the target, then
//   the target's bounds lie inside the viewport. Fails at the reach step
//   today (no stop), so visibility is not reached; once the stop exists it
//   fails at visibility until the reveal exists.
// * V3 PAINT: the ring tagged `{id}-ring` lies inside the viewport too.
// The 1600px-tall tests above stay as the non-scrolling controls.

const NARROW: (f32, f32) = (960., 420.);
const REVEAL_TARGET: &str = "window-opacity";

fn open_narrow(
    cx: &mut TestAppContext,
    section: SettingsSection,
) -> (Entity<Tty59App>, VisualTestContext) {
    let (app, mut vcx) = harness(cx);
    // FIXTURE: the reveal is a focus-in listener, and gpui builds the focus
    // events those listeners act on from empty paths while a window is inactive
    // (a fresh test window is), so the window is activated the supported way and
    // asserted, as the real app's window is when a person uses it.
    vcx.update(|w, _| w.activate_window());
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| w.is_window_active()),
        "setup: the test window is active, so focus-in listeners can fire"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(section, window, cx);
    });
    vcx.simulate_resize(size(px(NARROW.0), px(NARROW.1)));
    settle(&mut vcx);
    (app, vcx)
}

fn viewport(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> gpui::Bounds<gpui::Pixels> {
    state(app, vcx, |s| s.content_scroll.bounds())
}

fn offset_y(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> gpui::Pixels {
    state(app, vcx, |s| s.content_scroll.offset().y)
}

/// `inner` lies inside `outer` (half a pixel of slack for layout rounding).
fn within(inner: gpui::Bounds<gpui::Pixels>, outer: gpui::Bounds<gpui::Pixels>) -> bool {
    let slack = px(0.5);
    inner.top() >= outer.top() - slack
        && inner.bottom() <= outer.bottom() + slack
        && inner.left() >= outer.left() - slack
        && inner.right() <= outer.right() + slack
}

#[gpui::test]
fn v0_in_a_narrow_window_the_target_starts_below_the_viewport(cx: &mut TestAppContext) {
    let (app, mut vcx) = open_narrow(cx, SettingsSection::Appearance);
    let view = viewport(&app, &mut vcx);
    assert!(
        view.size.height > px(100.),
        "setup: the scroll viewport is laid out ({view:?})"
    );
    assert_eq!(
        offset_y(&app, &mut vcx),
        px(0.),
        "setup: the page starts at the top"
    );
    let target = vcx
        .debug_bounds(REVEAL_TARGET)
        .expect("the target is laid out even where it is clipped");
    assert!(
        target.top() > view.bottom(),
        "setup: {REVEAL_TARGET} {target:?} starts below the viewport {view:?}, so a tall window cannot mask the reveal"
    );
}

#[gpui::test]
fn v1_tab_to_a_control_below_the_fold_brings_it_into_view(cx: &mut TestAppContext) {
    let (app, mut vcx) = open_narrow(cx, SettingsSection::Appearance);
    let view = viewport(&app, &mut vcx);
    let before = vcx.debug_bounds(REVEAL_TARGET).expect("laid out");
    assert!(
        before.top() > view.bottom(),
        "setup: starts below the viewport"
    );

    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, REVEAL_TARGET),
        "FOCUS: Tab never reached {REVEAL_TARGET}"
    );
    let after = vcx.debug_bounds(REVEAL_TARGET).expect("still laid out");
    let view = viewport(&app, &mut vcx);
    assert!(
        offset_y(&app, &mut vcx) < px(0.),
        "VISIBLE: the page scrolled down to the focused control"
    );
    assert!(
        within(after, view),
        "VISIBLE: the focused {REVEAL_TARGET} {after:?} lies inside the viewport {view:?}"
    );
}

#[gpui::test]
fn v3_the_ring_of_a_revealed_stop_is_inside_the_viewport(cx: &mut TestAppContext) {
    let (app, mut vcx) = open_narrow(cx, SettingsSection::Appearance);
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, REVEAL_TARGET),
        "FOCUS: Tab never reached {REVEAL_TARGET}"
    );
    let view = viewport(&app, &mut vcx);
    let ring = vcx
        .debug_bounds("window-opacity-ring")
        .expect("PAINT: the focused stop draws no ring");
    assert!(
        within(ring, view),
        "PAINT: the ring {ring:?} lies inside the viewport {view:?}, not cut by its edge"
    );
}

// ------------------------------------------------------------------ R13 forwards (real Switch under FORWARD_SWITCH_CONTEXT)
//
// The real panel row, over a fixture daemon on the app's own transport (the
// wire_tests pattern, own copy: no cross-import). Each case re-runs itself in
// a child process so the socket and config dir are its own.

mod forwards {
    use super::*;
    use crate::daemon::protocol::{
        ClientMsg, DaemonMsg, ForwardStatus, ManagedForward, SshForwardKind,
    };
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::forwards::{edit_requests, remove_requests, row_handles};
    use crate::ui::right_panel::test_panel::open_info;

    const CHILD: &str = "TTY59_SETTINGS_KBD_WIRE_CHILD";
    const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

    fn passed_count(stdout: &str) -> Option<u32> {
        stdout.lines().find_map(|line| {
            let rest = line.strip_prefix("test result: ok. ")?;
            rest.split_once(" passed;")?.0.trim().parse().ok()
        })
    }

    /// In the parent: run `case` again, alone, in a child, and require that it
    /// ran and passed. In the child: return `true` and let the case go on.
    fn in_child(case: &str) -> bool {
        use std::io::Read;
        use std::process::Stdio;

        if std::env::var_os(CHILD).is_some() {
            return true;
        }
        let module = module_path!()
            .split_once("::")
            .map(|(_, rest)| rest)
            .expect("a module path has a crate");
        let name = format!("{module}::{case}");
        let mut child = Command::new(std::env::current_exe().expect("locate the test binary"))
            .args(["--exact", &name, "--nocapture"])
            .env(CHILD, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the child test process");
        let drain = |mut pipe: Box<dyn Read + Send>| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = pipe.read_to_end(&mut bytes);
                String::from_utf8_lossy(&bytes).into_owned()
            })
        };
        let stdout = drain(Box::new(child.stdout.take().expect("piped stdout")));
        let stderr = drain(Box::new(child.stderr.take().expect("piped stderr")));
        let give_up = Instant::now() + CHILD_TIMEOUT;
        let status = loop {
            match child.try_wait().expect("poll the child") {
                Some(status) => break Some(status),
                None if Instant::now() >= give_up => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        let stdout = stdout.join().expect("collect the child's stdout");
        let stderr = stderr.join().expect("collect the child's stderr");
        let Some(status) = status else {
            panic!("child {name} timed out and was killed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
        };
        assert!(
            status.success(),
            "child {name} failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert_eq!(
            passed_count(&stdout),
            Some(1),
            "child {name} did not run exactly one test:\n{stdout}"
        );
        false
    }

    #[derive(Default)]
    struct State {
        forwards: Vec<ManagedForward>,
        requests: Vec<String>,
        failures: Vec<String>,
    }

    struct Fixture {
        state: Arc<Mutex<State>>,
        stop: Arc<AtomicBool>,
        server: Option<JoinHandle<()>>,
    }

    impl Fixture {
        fn start(forwards: Vec<ManagedForward>) -> Fixture {
            crate::core::config::pin_test_config_dir();
            let dir = tty59_core::core::config::config_dir_path().expect("a config dir");
            assert_eq!(
                dir,
                std::env::temp_dir().join(format!("tty59-covtest-{}", std::process::id())),
                "the child's config dir is its own"
            );
            crate::daemon::transport::remove_stale_endpoint();
            let listener = crate::daemon::transport::bind().expect("bind the fixture daemon");
            // The endpoint only exists once bound: a Unix socket path is fixed
            // by the config dir, a Windows endpoint is the loopback port the
            // listener was given, published in the dir's port file.
            #[cfg(unix)]
            {
                let socket = dir.join("daemon.sock");
                assert_eq!(
                    crate::daemon::transport::endpoint_display(),
                    socket.display().to_string(),
                    "the transport dials the socket in that dir"
                );
                assert_eq!(
                    listener
                        .local_addr()
                        .expect("the listener has an address")
                        .as_pathname(),
                    Some(socket.as_path()),
                    "the listener is bound to the socket in that dir"
                );
            }
            #[cfg(windows)]
            {
                let port = listener
                    .local_addr()
                    .expect("the listener has an address")
                    .port();
                assert_eq!(
                    crate::daemon::transport::endpoint_display(),
                    format!("127.0.0.1:{port}"),
                    "the transport dials the port the listener holds"
                );
                let published = std::fs::read_to_string(dir.join("daemon.port"))
                    .expect("the port file is published in that dir");
                assert_eq!(
                    published.lines().next().map(str::trim),
                    Some(port.to_string().as_str()),
                    "the port file in that dir names the listener's port"
                );
            }
            // Routing proof: the production dialer lands on this very listener
            // and passes the transport's own authentication.
            listener
                .set_nonblocking(true)
                .expect("the listener goes non-blocking");
            let probe = crate::daemon::transport::connect()
                .expect("the production dialer reaches the fixture daemon");
            let give_up = Instant::now() + Duration::from_secs(2);
            let mut accepted = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < give_up => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("the dialed connection never reached the listener: {e}"),
                }
            };
            accepted
                .set_nonblocking(false)
                .and_then(|()| accepted.set_read_timeout(Some(Duration::from_secs(2))))
                .expect("bound the probe connection's I/O");
            crate::daemon::transport::authenticate(&mut accepted)
                .expect("the dialed connection authenticates against the listener");
            drop(accepted);
            drop(probe);
            let state = Arc::new(Mutex::new(State {
                forwards,
                ..State::default()
            }));
            let stop = Arc::new(AtomicBool::new(false));
            let server = std::thread::spawn({
                let (state, stop) = (state.clone(), stop.clone());
                move || serve(listener, state, stop)
            });
            Fixture {
                state,
                stop,
                server: Some(server),
            }
        }

        fn requests(&self) -> Vec<String> {
            self.state.lock().unwrap().requests.clone()
        }

        fn finish(mut self) {
            self.shutdown();
            let state = self.state.lock().unwrap();
            assert!(
                state.failures.is_empty(),
                "the fixture daemon hit transport failures: {:?}",
                state.failures
            );
        }

        fn shutdown(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(server) = self.server.take() {
                server.join().expect("the fixture daemon thread joined");
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    fn serve(
        listener: crate::daemon::transport::Listener,
        state: Arc<Mutex<State>>,
        stop: Arc<AtomicBool>,
    ) {
        let give_up = Instant::now() + Duration::from_secs(120);
        while !stop.load(Ordering::SeqCst) && Instant::now() < give_up {
            match listener.accept() {
                Ok((mut stream, _)) => answer(&mut stream, &state),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => {
                    state.lock().unwrap().failures.push(format!("accept: {e}"));
                    return;
                }
            }
        }
    }

    fn answer(stream: &mut crate::daemon::transport::Stream, state: &Arc<Mutex<State>>) {
        let fail = |what: String| state.lock().unwrap().failures.push(what);
        if let Err(e) = stream
            .set_nonblocking(false)
            .and_then(|()| stream.set_read_timeout(Some(Duration::from_secs(2))))
            .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(2))))
        {
            return fail(format!("configure the connection: {e}"));
        }
        // The production client sends its auth token first on Windows; Unix
        // authenticates nothing. A bad token is a failure, never skipped.
        if let Err(e) = crate::daemon::transport::authenticate(stream) {
            return fail(format!("authenticate the connection: {e}"));
        }
        let message = match ClientMsg::read(stream) {
            Ok(message) => message,
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => return,
            Err(e) => return fail(format!("read a request: {e}")),
        };
        let reply = {
            let mut state = state.lock().unwrap();
            match message {
                ClientMsg::ListForwards { pane_id } => {
                    state.requests.push(format!("list:{pane_id}"));
                    DaemonMsg::ForwardList(state.forwards.clone())
                }
                ClientMsg::RemoveForward {
                    pane_id,
                    forward_id,
                } => {
                    state
                        .requests
                        .push(format!("remove:{pane_id}:{forward_id}"));
                    state.forwards.retain(|f| f.id != forward_id);
                    DaemonMsg::ForwardList(state.forwards.clone())
                }
                ClientMsg::SetForwardEnabled {
                    pane_id,
                    forward_id,
                    enabled,
                } => {
                    state
                        .requests
                        .push(format!("enable:{pane_id}:{forward_id}:{enabled}"));
                    for f in state.forwards.iter_mut().filter(|f| f.id == forward_id) {
                        f.enabled = enabled;
                    }
                    DaemonMsg::ForwardList(state.forwards.clone())
                }
                _ => {
                    state.requests.push("other".into());
                    DaemonMsg::Error("the fixture does not serve this".into())
                }
            }
        };
        use std::io::Write as _;
        if let Err(e) = reply.encode(stream).and_then(|()| stream.flush()) {
            fail(format!("write a reply: {e}"));
        }
    }

    fn forward(id: u64) -> ManagedForward {
        ManagedForward {
            id,
            pane_id: 1,
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            bind_port: 8000 + id as u16,
            target_host: "10.0.0.5".into(),
            target_port: 80,
            description: None,
            status: ForwardStatus::Listening,
            enabled: true,
        }
    }

    fn enabled_of(app: &Entity<Tty59App>, vcx: &VisualTestContext) -> Vec<(u64, bool)> {
        app.read_with(vcx, |app, _| {
            app.loopback_panel
                .managed
                .iter()
                .map(|f| (f.id, f.enabled))
                .collect()
        })
    }

    fn app_on(
        cx: &mut TestAppContext,
        fixture: &Fixture,
        listed: &[u64],
    ) -> (Entity<Tty59App>, VisualTestContext) {
        row_handles::clear();
        remove_requests::take();
        edit_requests::take();
        let live = crate::terminal::RemoteTerminal::list_forwards(1);
        assert_eq!(
            live.iter().map(|f| f.id).collect::<Vec<_>>(),
            listed,
            "control: the fixture answers the app's own transport"
        );
        assert_eq!(fixture.requests(), vec!["list:1".to_string()]);
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, Vec::new(), live);
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_width.set(260.);
            cx.notify();
        });
        settle(&mut vcx);
        edit_requests::take();
        (app, vcx)
    }

    fn focus_row(vcx: &mut VisualTestContext, id: u64) {
        let handle = row_handles::get(id).expect("the row was drawn");
        vcx.update(|window, cx| window.focus(&handle, cx));
        settle(vcx);
    }

    fn focused_row(vcx: &mut VisualTestContext, among: &[u64]) -> Option<u64> {
        vcx.update(|window, cx| {
            let focused = window.focused(cx)?;
            among
                .iter()
                .find(|id| row_handles::get(**id).is_some_and(|h| h == focused))
                .copied()
        })
    }

    fn inside_row(vcx: &mut VisualTestContext, id: u64) -> bool {
        let handle = row_handles::get(id).expect("the row was drawn");
        vcx.update(|window, cx| handle.contains_focused(window, cx))
    }

    /// Tab from row 9 to its switch (marker `panel-forward-enabled-9`, the
    /// real `Switch` under the `ForwardSwitch` context); stops short of row 10.
    #[gpui::test]
    fn r13_tab_from_a_row_reaches_its_own_switch_and_space_and_enter_send_set_enabled(
        cx: &mut TestAppContext,
    ) {
        if !in_child(
            "r13_tab_from_a_row_reaches_its_own_switch_and_space_and_enter_send_set_enabled",
        ) {
            return;
        }
        let fixture = Fixture::start(vec![forward(9), forward(10)]);
        let (app, mut vcx) = app_on(cx, &fixture, &[9, 10]);
        focus_row(&mut vcx, 9);
        assert_eq!(focused_row(&mut vcx, &[9, 10]), Some(9), "setup: on row 9");

        let mut reached = false;
        for _ in 0..4 {
            if marker(&mut vcx).as_deref() == Some("panel-forward-enabled-9") {
                reached = true;
                break;
            }
            tab(&mut vcx);
            assert!(
                inside_row(&mut vcx, 9) || marker(&mut vcx).is_some(),
                "Tab left row 9 before reaching its switch"
            );
            assert_ne!(
                focused_row(&mut vcx, &[10]),
                Some(10),
                "Tab reached row 10 before row 9's switch"
            );
        }
        assert!(reached, "Tab never reached row 9's switch");

        press(&mut vcx, "space");
        assert_eq!(
            fixture.requests(),
            vec!["list:1".to_string(), "enable:1:9:false".to_string()],
            "Space sent exactly one SetForwardEnabled, for its own forward, off"
        );
        assert_eq!(
            enabled_of(&app, &vcx),
            vec![(9, false), (10, true)],
            "same row only"
        );
        assert!(inside_row(&mut vcx, 9), "focus stays in the row");
        press(&mut vcx, "enter");
        assert_eq!(
            fixture.requests().last().map(String::as_str),
            Some("enable:1:9:true"),
            "Enter sent exactly one more, back on"
        );
        assert_eq!(enabled_of(&app, &vcx), vec![(9, true), (10, true)]);
        assert_eq!(
            edit_requests::take(),
            Vec::<(u64, u64)>::new(),
            "the switch's keys reached the row's edit handler zero times"
        );
        assert_eq!(fixture.requests().len(), 3, "and sent nothing else");
        fixture.finish();
    }

    /// With the switch focused, Delete is the row's removal; the walk off the
    /// removed row steps past its own parts onto the neighbour row.
    #[gpui::test]
    fn r13b_removing_from_the_switch_lands_on_the_neighbour_row(cx: &mut TestAppContext) {
        if !in_child("r13b_removing_from_the_switch_lands_on_the_neighbour_row") {
            return;
        }
        let fixture = Fixture::start(vec![forward(9), forward(10)]);
        let (app, mut vcx) = app_on(cx, &fixture, &[9, 10]);
        focus_row(&mut vcx, 9);
        let mut reached = false;
        for _ in 0..4 {
            if marker(&mut vcx).as_deref() == Some("panel-forward-enabled-9") {
                reached = true;
                break;
            }
            tab(&mut vcx);
        }
        assert!(reached, "Tab never reached row 9's switch");

        vcx.simulate_keystrokes("delete");
        settle(&mut vcx);
        assert_eq!(
            fixture.requests(),
            vec!["list:1".to_string(), "remove:1:9".to_string()],
            "Delete on the switch removed its own row, once"
        );
        let ids: Vec<u64> = enabled_of(&app, &vcx)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec![10]);
        assert_eq!(
            focused_row(&mut vcx, &[10]),
            Some(10),
            "focus is on the neighbour row itself, not on a part of it"
        );
        fixture.finish();
    }
}
