//! A focused control that unmounts must not leave the settings page without a
//! keyboard. The mechanism is already in the app, not in the controls: when the
//! focused element is no longer drawn, gpui reports focus lost at the end of the
//! draw, and `Tty59App`'s `on_focus_lost` (app.rs) runs `focus_active`, which
//! with settings open focuses the page's own root handle. These tests drive that
//! with the real app, real keys, and no hand focus after the unmount:
//!
//! * `c0` (passes today): an EXISTING focusable that unmounts (a Keybindings
//!   page control) -> focus is on the mounted settings root -> a real Tab moves
//!   on -> a real Escape closes settings.
//! * `s2a`, `s2b` (downstream, red until the Back link and the per-row Reset
//!   are stops): the same for a Back link activated by Enter and a Reset whose
//!   row disappears when it is pressed.
//!
//! Across the unmount only a Weak handle is held, so a test never keeps the old
//! handle alive.

use super::kit::test_support::harness;
use super::*;
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

/// `debug_bounds` wants a `&'static str`; a test leaks the one it builds.
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

/// What must hold after the focused control has unmounted: the old handle is
/// gone (only a Weak was kept), focus is on the mounted settings root, a real
/// Tab moves focus on to another stop. The callers then press the real
/// Escape(s) the page owns.
fn the_keyboard_survives(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    old: &gpui::WeakFocusHandle,
    what: &str,
) {
    assert!(
        old.upgrade().is_none(),
        "{what}: the unmounted control's handle was dropped, not kept"
    );
    let root = state(app, vcx, |s| s.focus_handle.clone());
    assert!(
        focused(vcx) == Some(root.clone()),
        "{what}: focus fell back to the mounted settings root"
    );
    tab(vcx);
    let after = focused(vcx);
    assert!(
        after.is_some() && after != Some(root),
        "{what}: a real Tab from the settings root moved focus on"
    );
}

/// c0: an existing control unmounts under focus; nothing else in the app is
/// involved.
#[gpui::test]
fn c0_a_focused_control_that_unmounts_hands_focus_to_the_settings_root(cx: &mut TestAppContext) {
    use gpui::Focusable as _;
    let (app, mut vcx) = open(cx, SettingsSection::Keybindings);
    let filter = app.update_in(&mut vcx, |app, window, cx| {
        let f = app.active_settings().unwrap().shortcut_search.clone();
        f.update(cx, |f, cx| f.focus(window, cx));
        f.read(cx).focus_handle(cx)
    });
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| filter.is_focused(w)),
        "setup: the shortcut filter holds focus"
    );
    // Real Tabs onward from the filter, PAST the kit's marked stops (buttons,
    // switches: never activated here) and the page's own Inputs and root, to the
    // first handle that is none of those. Whether that is a shortcut CELL is
    // proved by the Enter / recording checks below, not assumed from the walk.
    let skipped: Vec<gpui::FocusHandle> = app.update_in(&mut vcx, |app, _, cx| {
        let s = app.active_settings().unwrap();
        vec![
            filter.clone(),
            s.search.read(cx).focus_handle(cx),
            s.shortcut_search.read(cx).focus_handle(cx),
            s.menu_query.read(cx).focus_handle(cx),
            s.focus_handle.clone(),
        ]
    });
    let mut walked = None;
    for _ in 0..MAX_TABS {
        tab(&mut vcx);
        let Some(now) = focused(&mut vcx) else {
            break;
        };
        if marker(&mut vcx).is_some() || skipped.contains(&now) {
            continue;
        }
        walked = Some(now);
        break;
    }
    let on_page = walked.expect("setup: Tab never reached an unmarked page control");
    assert!(on_page != filter, "setup: the Tab left the filter");

    // It must be a page-owned shortcut CELL, not merely something else: Enter
    // on a cell starts a recording for its own action, and that action's cell
    // is drawn on this page. The same handle holds focus through it and after
    // Escape cancels it.
    press(&mut vcx, "enter");
    let recording = state(&app, &mut vcx, |s| {
        s.recording
            .as_ref()
            .map(|r| (r.action.clone(), r.chords.clone()))
    });
    let (action, chords) = recording.expect(
        "setup: Enter on the focused control started a recording, so it is a shortcut cell",
    );
    assert!(
        chords.is_empty(),
        "setup: nothing is recorded yet: {chords:?}"
    );
    assert!(
        vcx.debug_bounds(leak(format!("kb-{action}"))).is_some(),
        "setup: the recording is for {action}, whose cell is drawn on this page"
    );
    assert!(
        focused(&mut vcx) == Some(on_page.clone()),
        "setup: the same cell holds focus while it records"
    );
    press(&mut vcx, "escape");
    assert!(
        state(&app, &mut vcx, |s| s.recording.is_none()),
        "setup: Escape cancels the recording"
    );
    assert!(
        settings_open(&app, &mut vcx),
        "setup: that Escape went to the recorder, not the page"
    );
    assert!(
        focused(&mut vcx) == Some(on_page.clone()),
        "setup: the same cell is still focused after the recording"
    );

    // From here only a Weak is kept.
    let weak = on_page.downgrade();
    drop(on_page);
    drop(filter);

    // The page changes under that focus, by the call the Back link makes.
    app.update_in(&mut vcx, |app, window, cx| {
        app.navigate_settings(SettingsSection::General, None, window, cx);
    });
    settle(&mut vcx);
    assert!(
        state(&app, &mut vcx, |s| s.section) == SettingsSection::General,
        "setup: the page changed"
    );

    the_keyboard_survives(&app, &mut vcx, &weak, "c0");
    press(&mut vcx, "escape");
    assert!(
        !settings_open(&app, &mut vcx),
        "c0: a real Escape still closes settings (no filter, no menu is open)"
    );
}

/// s2a: Enter on the Back link; the link is gone with the Keybindings page.
#[gpui::test]
fn s2a_back_link_activated_by_enter_leaves_a_working_keyboard(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::Keybindings);
    focus_search(&app, &mut vcx);
    assert!(
        tab_until(&mut vcx, |m| m == "settings-back").is_some(),
        "Tab never reached settings-back"
    );
    let weak = focused(&mut vcx)
        .expect("the back link holds focus")
        .downgrade();

    press(&mut vcx, "enter");
    assert!(
        state(&app, &mut vcx, |s| s.section) == SettingsSection::KeyboardMouse,
        "Enter on Back went to Keyboard & Mouse"
    );
    assert!(
        vcx.debug_bounds("settings-back").is_none(),
        "setup: the Back link is gone from that page"
    );

    the_keyboard_survives(&app, &mut vcx, &weak, "s2a");
    press(&mut vcx, "escape");
    assert!(
        !settings_open(&app, &mut vcx),
        "s2a: a real Escape closes settings"
    );
}

/// s2b: Enter on a Reset whose row leaves the modified-only list when it is
/// pressed.
#[gpui::test]
fn s2b_a_reset_that_removes_its_own_row_leaves_a_working_keyboard(cx: &mut TestAppContext) {
    let (app, mut vcx) = open(cx, SettingsSection::KeyboardMouse);
    let default = Config::default().mouse_hide_while_typing;
    // The click path makes the row modified; the filter then lists only
    // modified rows, so resetting it empties the list.
    click(&mut vcx, "term-mouse-hide");
    assert!(
        vcx.update(|_, cx| cx.global::<Config>().mouse_hide_while_typing) != default,
        "setup: the row is modified"
    );
    app.update_in(&mut vcx, |app, _, cx| {
        app.active_settings_mut().unwrap().modified_only = true;
        app.autoselect_settings_search(cx);
    });
    settle(&mut vcx);
    focus_search(&app, &mut vcx);
    assert!(
        tab_until(&mut vcx, |m| m.starts_with("reset-setting-")).is_some(),
        "Tab never reached a per-row Reset"
    );
    let weak = focused(&mut vcx)
        .expect("the Reset holds focus")
        .downgrade();

    press(&mut vcx, "enter");
    assert!(
        vcx.update(|_, cx| cx.global::<Config>().mouse_hide_while_typing) == default,
        "setup: Enter reset the value"
    );

    the_keyboard_survives(&app, &mut vcx, &weak, "s2b");
    // The filter is still on: the first Escape peels it, the second closes.
    press(&mut vcx, "escape");
    assert!(
        settings_open(&app, &mut vcx) && !state(&app, &mut vcx, |s| s.modified_only),
        "s2b: the first real Escape cleared the modified-only filter and left settings open"
    );
    press(&mut vcx, "escape");
    assert!(
        !settings_open(&app, &mut vcx),
        "s2b: the second real Escape closes settings"
    );
}
