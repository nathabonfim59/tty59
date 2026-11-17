//! Real-page keyboard reach for controls that are not Btn / Switch / dropdown:
//! the theme mode cards, the agents "more" trigger, the mobile switch, and the
//! shape of the keyboard ring on a nav item. Real settings page, real Tab / Enter
//! / Space, the real callbacks; selectors and context strings only (the ring is
//! found as `{id}-ring`).
//!
//! Positive controls (`p*`) pass on today's tree: they show the fixtures mount the
//! control and that the callback the keyboard is expected to reach is the one a
//! pointer click reaches. The reach tests t1, t2 and n1 are red until their
//! controls are stops. t3 is NOT a promised RED: the mobile switch is already a
//! `kit::Switch` with a handler, so it is a stop from the kit core on; it is a
//! real-page reach control for that, and it never activates the switch (mobile
//! activation is untested here; see its doc).

use super::kit::test_support::harness;
use super::*;
use crate::core::agent_hooks::{HookAgent, HooksState};
use gpui::{KeyUpEvent, Keystroke, Modifiers, TestAppContext, VisualTestContext, size};

const MAX_TABS: usize = 160;

fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..3 {
        vcx.run_until_parked();
        vcx.update(|w, cx| {
            let _ = w.draw(cx);
        });
    }
}

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

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
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

fn focus_search(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
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
}

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

fn near(a: gpui::Bounds<gpui::Pixels>, b: gpui::Bounds<gpui::Pixels>) -> bool {
    let close = |x: gpui::Pixels, y: gpui::Pixels| (x - y).abs() <= px(0.5);
    close(a.origin.x, b.origin.x)
        && close(a.origin.y, b.origin.y)
        && close(a.size.width, b.size.width)
        && close(a.size.height, b.size.height)
}

/// `b` moved in by `by` on every side.
fn shrunk(b: gpui::Bounds<gpui::Pixels>, by: gpui::Pixels) -> gpui::Bounds<gpui::Pixels> {
    gpui::Bounds {
        origin: gpui::point(b.origin.x + by, b.origin.y + by),
        size: size(b.size.width - by * 2.0, b.size.height - by * 2.0),
    }
}

// ------------------------------------------------------------------ theme mode cards

/// The theme the cards leave behind when System is the mode: a click on the Dark
/// card pins Dark. (`set_theme_mode`: follow-system off, the Dark preset.)
fn system_mode_page(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        cx.global_mut::<Config>().theme_follow_system = true;
        app.open_settings_section(SettingsSection::Appearance, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    settle(&mut vcx);
    (app, vcx)
}

fn dark_card_pins_dark(vcx: &mut VisualTestContext) -> bool {
    cfg(vcx, |c| {
        !c.theme_follow_system && c.theme_preset == c.theme_preset_dark
    })
}

/// p6: the Dark card is drawn, and a click on it is the activation the keyboard
/// must reach: follow-system off and the Dark preset.
#[gpui::test]
fn p6_a_click_on_the_dark_theme_card_pins_dark(cx: &mut TestAppContext) {
    let (_app, mut vcx) = system_mode_page(cx);
    assert!(!dark_card_pins_dark(&mut vcx), "setup: the mode is System");
    click(&mut vcx, "theme-mode-Dark");
    assert!(dark_card_pins_dark(&mut vcx), "the click pins Dark");
}

#[gpui::test]
fn t1_tab_reaches_a_theme_mode_card_and_enter_picks_it(cx: &mut TestAppContext) {
    let (app, mut vcx) = system_mode_page(cx);
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "theme-mode-Dark"),
        "Tab never reached the Dark theme card"
    );
    press(&mut vcx, "enter");
    assert!(
        dark_card_pins_dark(&mut vcx),
        "Enter on the card did what its click does"
    );
}

// ------------------------------------------------------------------ agents "more" trigger

/// An Agents page whose first agent has hooks installed, so its row has the
/// "more" menu: the same state injection the page's paint tests use (the page
/// otherwise reads the machine's hook files).
fn agents_page(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Agents, window, cx);
        let s = app.active_settings_mut().unwrap();
        s.agent_hooks_seq += 1;
        s.agent_show_all = true;
        s.agent_hooks_states = AgentHooksView::Ready(
            HookAgent::ALL
                .iter()
                .enumerate()
                .map(|(i, &agent)| AgentHookRow {
                    agent,
                    state: if i == 0 {
                        HooksState::Installed
                    } else {
                        HooksState::NotInstalled
                    },
                    target: format!("~/.agent-{i}/hooks.json"),
                })
                .collect(),
        );
        cx.notify();
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    settle(&mut vcx);
    (app, vcx)
}

/// p7: the row's trigger is drawn and a click on it opens its menu.
#[gpui::test]
fn p7_a_click_on_the_agent_more_trigger_opens_its_menu(cx: &mut TestAppContext) {
    let (app, mut vcx) = agents_page(cx);
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()),
        "setup: no menu is open"
    );
    click(&mut vcx, "agent-menu-0-trigger");
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu.clone()).as_deref(),
        Some("agent-menu-0"),
        "the click opens the row's menu"
    );
}

#[gpui::test]
fn t2_tab_reaches_the_agent_more_trigger_and_enter_opens_its_menu(cx: &mut TestAppContext) {
    let (app, mut vcx) = agents_page(cx);
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "agent-menu-0-trigger"),
        "Tab never reached the agent more trigger"
    );
    press(&mut vcx, "enter");
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu.clone()).as_deref(),
        Some("agent-menu-0"),
        "Enter opens the menu, and the same key-up did not close it"
    );
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("agent-menu-0-trigger"),
        "focus stays on the trigger"
    );
}

// ------------------------------------------------------------------ mobile switch

/// p8: the mobile switch is drawn. (Pressing it asks for phone access, which
/// the page then waits on, so the control here is only that it is mounted.)
#[gpui::test]
fn p8_the_mobile_access_switch_is_drawn_and_off(cx: &mut TestAppContext) {
    let (_app, mut vcx) = open(cx, SettingsSection::Mobile);
    assert!(
        vcx.debug_bounds("mobile-access").is_some(),
        "the mobile access switch is drawn"
    );
    assert!(
        !cfg(&mut vcx, |c| c.mobile_access),
        "setup: phone access is off"
    );
}

/// t3: real-page reach control (expected GREEN with the kit core, where the
/// mobile switch is already a stop): a real Tab lands on the switch, named by the
/// live marker and holding focus. It does NOT activate it. Switching phone access
/// on spawns a detached, timer-driven gateway wait (mobile.rs, `.detach()`, 500ms
/// polls for up to 15s, state files under the process-shared test config dir),
/// which is timing-dependent work this keyboard test must not start, so mobile
/// ACTIVATION IS UNTESTED HERE. What a key does to a `kit::Switch` is proved by R1
/// (Space and Enter on term-mouse-hide) and, over a real transport, by R13.
#[gpui::test]
fn t3_tab_reaches_the_mobile_switch_without_activating_it(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Mobile);
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "mobile-access"),
        "Tab never reached the mobile access switch"
    );
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some("mobile-access"),
        "the live focus marker names the mobile switch"
    );
    assert!(focused(&mut vcx).is_some(), "and a handle holds focus");
    assert!(
        !cfg(&mut vcx, |c| c.mobile_access) && !state(&app, &mut vcx, |s| s.mobile_starting),
        "reaching it asked for nothing: phone access is still off and nothing is starting"
    );
}

// ------------------------------------------------------------------ nav item ring

/// A nav item sits in a scrolling list with a 1px gap, so its keyboard ring is
/// drawn INSIDE its edge by the installed gap, where an outside ring would be
/// clipped or run into its neighbour. FOCUS first (a real Tab to a nav stop),
/// then PAINT: the `{id}-ring` bounds are the item's bounds moved in by the
/// installed focus-ring gap, and lie inside the item.
#[gpui::test]
fn n1_a_focused_nav_item_draws_its_ring_inside_its_edge(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::General);
    focus_search(&app, &mut vcx);
    let id = tab_until(&mut vcx, |m| m.starts_with("settings-nav-"))
        .expect("FOCUS: Tab never reached a nav item");
    let item = vcx
        .debug_bounds(leak(id.clone()))
        .expect("the focused nav item is drawn");
    let ring = vcx
        .debug_bounds(leak(format!("{id}-ring")))
        .expect("PAINT: a keyboard-focused nav item draws no ring");
    let gap = vcx.update(|_, cx| {
        use gpui_component::ActiveTheme as _;
        cx.theme().focus_ring_offset()
    });
    assert!(
        near(ring, shrunk(item, gap)),
        "the ring {ring:?} is the item {item:?} moved in by the installed gap {gap:?}"
    );
    assert!(
        ring.top() >= item.top()
            && ring.bottom() <= item.bottom()
            && ring.left() >= item.left()
            && ring.right() <= item.right(),
        "the ring {ring:?} lies inside the item {item:?}"
    );
}

// ------------------------------------------------------------------ dropdown arrows are plain keys

/// m3: a dropdown trigger answers the arrow keys only plain. With Ctrl, Alt,
/// Shift or Super held, Down neither opens the menu nor moves its highlight, so
/// the chord is left to whatever binds it; the plain Down still opens it.
#[gpui::test]
fn m3_a_modified_down_does_not_open_a_dropdown_but_the_plain_one_does(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Appearance);
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "cursor-style-trigger"),
        "Tab never reached the dropdown trigger"
    );
    for chord in ["ctrl-down", "alt-down", "shift-down", "cmd-down"] {
        press(&mut vcx, chord);
        assert!(
            state(&app, &mut vcx, |s| s.menu.is_none()),
            "{chord} opened the menu"
        );
    }
    press(&mut vcx, "down");
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_some()),
        "the plain Down opens it"
    );
    let hi = state(&app, &mut vcx, |s| s.menu_hi);
    for chord in ["ctrl-down", "alt-down", "shift-up", "cmd-end"] {
        press(&mut vcx, chord);
    }
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu_hi),
        hi,
        "no chord moved the highlight"
    );
}
