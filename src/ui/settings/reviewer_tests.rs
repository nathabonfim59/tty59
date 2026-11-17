//! Reviewer controls for two behaviours the stage-2/3 stops must have, written
//! BEFORE any production change for them (both are red on today's code):
//!
//! * `o1`: a stop's `on_key` is for the stop itself. A key that bubbles up from a
//!   focusable DESCENDANT (which does not consume it) is that descendant's, not
//!   the stop's.
//! * `m4`, `m4b`: a plain dropdown's open menu closes when real Tab / Shift-Tab
//!   move the focus off its trigger, without picking anything, and without the
//!   page forcing the focus back. (Root's Tab action runs before any key
//!   listener, so the trigger's own key handler never sees Tab.)
//!
//! A pointer variation (a click on another control with the menu open) is not
//! covered here: the popover scrim takes a background click first, by design.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::kit::test_support::harness as raw_harness;
use super::*;
use gpui::{KeyUpEvent, Keystroke, TestAppContext, VisualTestContext, size};

const MAX_TABS: usize = 160;

/// A test window is INACTIVE until the platform says otherwise
/// (`TestWindow::is_active` is false), and gpui builds the focus events its
/// `on_focus_in` / `on_focus_out` listeners act on from EMPTY paths while the
/// window is inactive (window.rs, Window::draw), so those listeners can never
/// fire in an inactive test window, whatever the code under test does. The
/// real app's window is active when a person uses it. Every window this module
/// drives is therefore made active the supported way (`activate_window`, which
/// queues the platform's active-status change), and the fixture asserts it: the
/// menu watch is a focus-out listener, and the tests that exercise it must run
/// in a window where such a listener can fire.
fn activate(vcx: &mut VisualTestContext) {
    vcx.update(|w, _| w.activate_window());
    settle(vcx);
    assert!(
        vcx.update(|w, _| w.is_window_active()),
        "setup: the test window is active, so focus-in/out listeners can fire"
    );
}

/// `harness`, with its window active.
fn harness(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext) {
    let (app, mut vcx) = raw_harness(cx);
    activate(&mut vcx);
    (app, vcx)
}

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

fn key_only(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
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

fn state<T>(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    f: impl FnOnce(&SettingsState) -> T,
) -> T {
    app.update_in(vcx, |app, _, _| {
        f(app.active_settings().expect("settings open"))
    })
}

fn open_appearance(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Appearance, window, cx);
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

fn tab_to(vcx: &mut VisualTestContext, id: &str) -> bool {
    let mut seen = vec![focused(vcx)];
    for _ in 0..MAX_TABS {
        if marker(vcx).as_deref() == Some(id) {
            return true;
        }
        key_only(vcx, "tab");
        let now = focused(vcx);
        if now.is_none() || seen.contains(&now) {
            return marker(vcx).as_deref() == Some(id);
        }
        seen.push(now);
    }
    false
}

// ------------------------------------------------------------------ o1: exact owner

struct Owner {
    stop_keys: Rc<Cell<usize>>,
    child_keys: Rc<RefCell<Vec<String>>>,
    child: gpui::FocusHandle,
}

impl Render for Owner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let stop_keys = self.stop_keys.clone();
        let child_keys = self.child_keys.clone();
        div().p(px(30.)).child(
            kit::stop(
                "owner",
                px(6.),
                div()
                    .id("owner")
                    .relative()
                    .w(px(160.))
                    .h(px(60.))
                    .p(px(8.))
                    .child(
                        div()
                            .id("owner-child")
                            .w(px(60.))
                            .h(px(24.))
                            .track_focus(&self.child.clone().tab_stop(true))
                            // Does not consume: the key bubbles on to the stop.
                            .on_key_down(move |ev, _, _| {
                                child_keys.borrow_mut().push(ev.keystroke.unparse())
                            }),
                    ),
            )
            .on_key(move |_, _, _| stop_keys.set(stop_keys.get() + 1)),
        )
    }
}

/// o1: the stop's own key callback is called for keys of the stop itself, never
/// for a key that was the focusable child's.
#[gpui::test]
fn o1_a_stops_key_callback_is_not_called_for_its_focusable_childs_keys(cx: &mut TestAppContext) {
    let (_app, _main) = harness(cx);
    let stop_keys = Rc::new(Cell::new(0usize));
    let child_keys = Rc::new(RefCell::new(Vec::<String>::new()));
    let (sk, ck) = (stop_keys.clone(), child_keys.clone());
    let child_slot = Rc::new(RefCell::new(None));
    let slot = child_slot.clone();
    let window = cx.add_window(move |_, cx| {
        let child = cx.focus_handle();
        *slot.borrow_mut() = Some(child.clone());
        Owner {
            stop_keys: sk,
            child_keys: ck,
            child,
        }
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(400.), px(200.)));
    settle(&mut vcx);
    let child = child_slot
        .borrow()
        .clone()
        .expect("the view built its child");

    // First stop in tree order is the stop itself.
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert!(
        !vcx.update(|w, _| child.is_focused(w)) && marker(&mut vcx).as_deref() == Some("owner"),
        "setup: the stop itself holds focus first"
    );
    press(&mut vcx, "right");
    assert_eq!(
        stop_keys.get(),
        1,
        "a key for the stop itself reaches its callback"
    );
    assert!(child_keys.borrow().is_empty(), "and the child saw nothing");

    // Then the focusable child inside it.
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| child.is_focused(w)),
        "setup: focus moved to the child inside the stop"
    );
    press(&mut vcx, "right");
    assert_eq!(
        child_keys.borrow().len(),
        1,
        "the child got its own key: {:?}",
        child_keys.borrow()
    );
    assert_eq!(
        stop_keys.get(),
        1,
        "OWNER: the stop's callback was also called for the child's key that bubbled through it"
    );
}

// ------------------------------------------------------------------ m4: open menu + Tab

fn open_cursor_menu(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> gpui::FocusHandle {
    focus_search(app, vcx);
    assert!(
        tab_to(vcx, "cursor-style-trigger"),
        "Tab never reached the dropdown trigger"
    );
    let trigger = focused(vcx).expect("the trigger holds focus");
    press(vcx, "enter");
    assert_eq!(
        state(app, vcx, |s| s.menu.clone()).as_deref(),
        Some("cursor-style"),
        "setup: Enter opened the menu"
    );
    assert!(
        focused(vcx) == Some(trigger.clone()),
        "setup: the trigger kept the focus while it is open"
    );
    trigger
}

/// m4: Tab off an open plain menu's trigger closes it, picks nothing, and leaves
/// the focus where Tab put it.
#[gpui::test]
fn m4_tab_off_an_open_dropdown_closes_it_without_picking(cx: &mut TestAppContext) {
    let (app, mut vcx) = open_appearance(cx);
    let before = vcx.update(|_, cx| cx.global::<Config>().cursor_style);
    let trigger = open_cursor_menu(&app, &mut vcx);

    key_only(&mut vcx, "tab");
    let after = focused(&mut vcx);
    assert!(
        after.is_some() && after != Some(trigger),
        "FOCUS: a real Tab moved focus off the trigger"
    );
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()),
        "the menu is still open after Tab moved focus away from its trigger"
    );
    assert_eq!(
        vcx.update(|_, cx| cx.global::<Config>().cursor_style),
        before,
        "closing by Tab picked nothing"
    );
    // The keyboard is still the page's: the next Tab moves on again.
    let here = focused(&mut vcx);
    key_only(&mut vcx, "tab");
    assert!(
        focused(&mut vcx) != here,
        "a second Tab still moves focus (nothing forced it back)"
    );
}

/// m4b: the same with Shift-Tab, which goes to the previous stop.
#[gpui::test]
fn m4b_shift_tab_off_an_open_dropdown_closes_it_without_picking(cx: &mut TestAppContext) {
    let (app, mut vcx) = open_appearance(cx);
    let before = vcx.update(|_, cx| cx.global::<Config>().cursor_style);
    let trigger = open_cursor_menu(&app, &mut vcx);

    key_only(&mut vcx, "shift-tab");
    let after = focused(&mut vcx);
    assert!(
        after.is_some() && after != Some(trigger),
        "FOCUS: a real Shift-Tab moved focus off the trigger"
    );
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()),
        "the menu is still open after Shift-Tab moved focus away from its trigger"
    );
    assert_eq!(
        vcx.update(|_, cx| cx.global::<Config>().cursor_style),
        before,
        "closing by Shift-Tab picked nothing"
    );
}

// ------------------------------------------------------------------ m4c: Down-opened menus too

/// m4c: a menu opened by the Down key (not Enter) closes on Tab the same way.
#[gpui::test]
fn m4c_tab_off_a_menu_opened_by_the_down_key_closes_it(cx: &mut TestAppContext) {
    let (app, mut vcx) = open_appearance(cx);
    let before = vcx.update(|_, cx| cx.global::<Config>().cursor_style);
    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "cursor-style-trigger"),
        "Tab never reached the dropdown trigger"
    );
    let trigger = focused(&mut vcx).expect("the trigger holds focus");
    press(&mut vcx, "down");
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu.clone()).as_deref(),
        Some("cursor-style"),
        "setup: Down opened the menu"
    );
    key_only(&mut vcx, "tab");
    assert!(
        focused(&mut vcx).is_some() && focused(&mut vcx) != Some(trigger),
        "FOCUS: a real Tab moved focus off the trigger"
    );
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()),
        "the menu a Down key opened is still open after Tab moved focus away"
    );
    assert_eq!(
        vcx.update(|_, cx| cx.global::<Config>().cursor_style),
        before,
        "nothing was picked"
    );
}

// ------------------------------------------------------------------ m5: the trigger disappears

/// An Agents page whose first agent has hooks installed (so it has the "more"
/// menu), by the same state injection the page's paint tests use.
fn agents_rows(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, first_installed: bool) {
    use crate::core::agent_hooks::{HookAgent, HooksState};
    app.update_in(vcx, |app, _, cx| {
        let s = app.active_settings_mut().unwrap();
        s.agent_hooks_seq += 1;
        s.agent_show_all = true;
        s.agent_hooks_states = AgentHooksView::Ready(
            HookAgent::ALL
                .iter()
                .enumerate()
                .map(|(i, &agent)| AgentHookRow {
                    agent,
                    state: if i == 0 && first_installed {
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
    settle(vcx);
}

/// m5: the trigger of an open menu is removed from the page (its row loses its
/// hooks) while the menu is open. The menu must not stay marked open with no
/// popover (where the first Escape would be spent on it), the keyboard must
/// still work, and one real Escape closes the page.
#[gpui::test]
fn m5_a_menu_whose_trigger_disappears_does_not_stay_open(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Agents, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    agents_rows(&app, &mut vcx, true);

    focus_search(&app, &mut vcx);
    assert!(
        tab_to(&mut vcx, "agent-menu-0-trigger"),
        "Tab never reached the agent more trigger"
    );
    press(&mut vcx, "enter");
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu.clone()).as_deref(),
        Some("agent-menu-0"),
        "setup: Enter opened the row's menu"
    );

    // The row loses its hooks: its trigger and popover leave the page.
    agents_rows(&app, &mut vcx, false);
    assert!(
        vcx.debug_bounds("agent-menu-0-trigger").is_none(),
        "setup: the trigger is gone from the page"
    );
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()),
        "the menu is still marked open though its trigger is gone"
    );

    // The keyboard still works: a real Tab moves focus on.
    let here = focused(&mut vcx);
    key_only(&mut vcx, "tab");
    assert!(
        focused(&mut vcx).is_some() && focused(&mut vcx) != here,
        "a real Tab still moves focus"
    );
    // And one real Escape closes the page: no leftover menu to peel first.
    press(&mut vcx, "escape");
    assert!(
        app.update_in(&mut vcx, |app, _, _| app.active_settings().is_none()),
        "one Escape closes settings (a stale menu would have taken it)"
    );
}

// ------------------------------------------------------------------ m6: a pick still works

/// m6 (control): Enter-open, Down, Enter picks the second row and closes, and a
/// following real Tab moves focus without reopening or changing anything.
#[gpui::test]
fn m6_a_keyboard_pick_still_works_and_a_following_tab_changes_nothing(cx: &mut TestAppContext) {
    use crate::core::config::CursorStyle;
    let (app, mut vcx) = open_appearance(cx);
    let trigger = open_cursor_menu(&app, &mut vcx);
    press(&mut vcx, "down");
    press(&mut vcx, "enter");
    assert!(
        vcx.update(|_, cx| cx.global::<Config>().cursor_style) == CursorStyle::Bar,
        "Enter picked the highlighted row"
    );
    assert!(state(&app, &mut vcx, |s| s.menu.is_none()), "and closed it");
    assert!(
        focused(&mut vcx) == Some(trigger.clone()),
        "the trigger kept the focus"
    );

    key_only(&mut vcx, "tab");
    assert!(
        focused(&mut vcx).is_some() && focused(&mut vcx) != Some(trigger),
        "a real Tab moves focus on"
    );
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()),
        "and no menu opened"
    );
    assert!(
        vcx.update(|_, cx| cx.global::<Config>().cursor_style) == CursorStyle::Bar,
        "and the pick stayed"
    );
}

// ------------------------------------------------------------------ m7: a stale queued close

/// m7 (post-implementation, DIRECT delivery, not the real deferred path): the
/// close a focus-out queued for one open must do nothing to a menu opened since
/// with the SAME id. The old open's weak token is taken, the menu is closed and
/// re-opened by real keys, then the old close is delivered by hand: it is
/// refused. Delivering the current open's weak token closes the menu, proving the
/// refusal is about the token and not a no-op.
#[gpui::test]
fn m7_a_queued_close_of_an_old_open_does_not_close_a_reopened_menu(cx: &mut TestAppContext) {
    let (app, mut vcx) = open_appearance(cx);
    open_cursor_menu(&app, &mut vcx);
    let old = app.update_in(&mut vcx, |app, _, _| {
        std::rc::Rc::downgrade(
            &app.active_settings()
                .and_then(|s| s.menu_watch.as_ref())
                .expect("a plain menu opened by Enter is watched")
                .token,
        )
    });

    // Close it (Escape) and open the SAME menu again (Enter).
    press(&mut vcx, "escape");
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()
            && s.menu_watch.is_none()),
        "setup: Escape closed the menu and ended its watch"
    );
    press(&mut vcx, "enter");
    assert_eq!(
        state(&app, &mut vcx, |s| s.menu.clone()).as_deref(),
        Some("cursor-style"),
        "setup: the same menu is open again"
    );
    let id = SharedString::from("cursor-style");

    // The old open's close arrives late.
    app.update_in(&mut vcx, |app, _, cx| {
        app.close_menu_after_blur(&id, &old, cx)
    });
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_some()),
        "a close queued by the OLD open closed the re-opened menu"
    );

    // The current open's own close does close it.
    let current = app.update_in(&mut vcx, |app, _, _| {
        std::rc::Rc::downgrade(
            &app.active_settings()
                .unwrap()
                .menu_watch
                .as_ref()
                .unwrap()
                .token,
        )
    });
    app.update_in(&mut vcx, |app, _, cx| {
        app.close_menu_after_blur(&id, &current, cx)
    });
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()
            && s.menu_watch.is_none()),
        "the current open's close closes it and drops its watch"
    );
}

// ------------------------------------------------------------------ m8: Ctrl/Cmd+F with a menu open

/// m8: the page's find chord with a plain menu open focuses the search box and
/// ends the menu and its watch; nothing then closes or refocuses anything late.
#[gpui::test]
fn m8_find_with_a_menu_open_focuses_search_and_ends_the_watch(cx: &mut TestAppContext) {
    use gpui::Focusable as _;
    let (app, mut vcx) = open_appearance(cx);
    open_cursor_menu(&app, &mut vcx);
    assert!(
        state(&app, &mut vcx, |s| s.menu_watch.is_some()),
        "setup: the open plain menu is watched"
    );

    press(&mut vcx, "secondary-f");
    let search = app.update_in(&mut vcx, |app, _, cx| {
        app.active_settings()
            .unwrap()
            .search
            .read(cx)
            .focus_handle(cx)
    });
    assert!(
        vcx.update(|w, _| search.is_focused(w)),
        "the find chord focused the search box"
    );
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()
            && s.menu_watch.is_none()
            && !s.menu_keyboard),
        "the menu, its keyboard highlight and its watch ended with it"
    );

    // Frames later (the focus-out of the trigger has been delivered): the
    // search box still holds focus, and nothing reopened or closed.
    settle(&mut vcx);
    settle(&mut vcx);
    assert!(
        vcx.update(|w, _| search.is_focused(w)),
        "no late close moved focus off the search box"
    );
    assert!(
        state(&app, &mut vcx, |s| s.menu.is_none()
            && s.menu_watch.is_none()),
        "and no stale watch is left"
    );
}

// ------------------------------------------------------------------ a1: the event witness

/// a1 (positive control for the fixture, not for the product): a real
/// `on_focus_out` listener on a real stop's handle does NOT fire in an inactive
/// test window and DOES fire once the window is activated, for the same two
/// focus moves. The counts are what the listener itself recorded.
#[gpui::test]
fn a1_a_focus_out_listener_fires_only_in_an_active_window(cx: &mut TestAppContext) {
    struct TwoStops;
    impl Render for TwoStops {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .p(px(30.))
                .child(kit::button("a-one", "One", kit::BtnKind::Secondary).on_click(|_, _, _| {}))
                .child(kit::button("a-two", "Two", kit::BtnKind::Secondary).on_click(|_, _, _| {}))
        }
    }

    // A themed app so the kit controls render; the window under test is its own.
    let (_app, _main) = raw_harness(cx);
    let window = cx.add_window(|_, _| TwoStops);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(400.), px(200.)));
    settle(&mut vcx);
    assert!(
        !vcx.update(|w, _| w.is_window_active()),
        "diagnostic: a fresh test window is inactive"
    );

    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    let first = focused(&mut vcx).expect("the first stop holds focus");
    let fired = Rc::new(Cell::new(0usize));
    let counter = fired.clone();
    let _watch = vcx
        .update(|w, cx| w.on_focus_out(&first, cx, move |_, _, _| counter.set(counter.get() + 1)));
    settle(&mut vcx);

    // Inactive: the focus really leaves the first stop, and nothing is heard.
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert!(
        focused(&mut vcx) != Some(first.clone()),
        "setup: focus left the first stop"
    );
    assert_eq!(
        fired.get(),
        0,
        "an INACTIVE window delivered a focus-out (the premise of the fixture correction is wrong)"
    );

    // Active: the same two moves are heard.
    activate(&mut vcx);
    vcx.update(|w, cx| w.focus(&first, cx));
    settle(&mut vcx);
    vcx.update(|w, cx| w.focus_next(cx));
    settle(&mut vcx);
    assert!(
        fired.get() >= 1,
        "an ACTIVE window delivered no focus-out when focus left the watched stop"
    );
}
