//! Keyboard reveal of a Keyboard Shortcuts cell in the real, separate
//! `SettingsWindow`. TESTS ONLY, written before the cell opts into the shared
//! reveal, so the reach test is expected RED until it does.
//!
//! The inline counterparts (`settings::cell_reveal_tests`) run the page in the
//! workspace window. These open the page the way the app does, through
//! `open_settings_window_then`, so the cell is drawn, focused and unmounted in a
//! second window with its own focus. This module is a child of `app` only so it
//! can reach that private function.
//!
//! * `Entity::update_in` follows the entity's registered window, which can be
//!   either one after their renders, so the settings window is always entered
//!   explicitly: `vcx.update(|window, cx| app.update(cx, ..))`;
//! * the settings window is ACTIVATED and asserted active (a reveal is a focus-in
//!   listener and an inactive test window delivers no focus events);
//! * the target is the nearest cell that starts entirely below the viewport,
//!   proved from its own debug bounds before any focus moves;
//! * Tab is a real key followed by `run_until_parked` only; which cell holds the
//!   keyboard is read from its `kb-ring-{action}` ring.

use super::Tty59App;
use crate::ui::settings::SettingsSection;
use crate::ui::settings::kit::test_support::harness;
use gpui::{
    Bounds, Entity, Focusable as _, KeyUpEvent, Keystroke, Pixels, TestAppContext,
    VisualTestContext, px, size,
};
use gpui_component::Root;

const MAX_TABS: usize = 400;
const NARROW: (f32, f32) = (960., 420.);

/// A Tty59App is shared by the workspace and SettingsWindow. Enter the requested
/// test window explicitly instead of following GPUI's registered window.
fn in_window<R>(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    f: impl FnOnce(&mut Tty59App, &mut gpui::Window, &mut gpui::Context<Tty59App>) -> R,
) -> R {
    vcx.update(|window, cx| app.update(cx, |app, cx| f(app, window, cx)))
}

fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..3 {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
}

/// A key down and up, then explicit draws: used only where the scroll is not the
/// claim.
fn press(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    vcx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).expect("a key name"),
    });
    settle(vcx);
}

fn within(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let slack = px(0.5);
    inner.top() >= outer.top() - slack
        && inner.bottom() <= outer.bottom() + slack
        && inner.left() >= outer.left() - slack
        && inner.right() <= outer.right() + slack
}

struct Cell {
    action: String,
    cell: &'static str,
    ring: &'static str,
}

struct Real {
    app: Entity<Tty59App>,
    /// Kept alive: dropping it closes the workspace window the page belongs to.
    _workspace: VisualTestContext,
    settings: VisualTestContext,
    cells: Vec<Cell>,
    target: usize,
}

impl Real {
    fn viewport(&mut self) -> Bounds<Pixels> {
        in_window(&self.app, &mut self.settings, |app, _, _| {
            app.active_settings()
                .expect("settings open")
                .content_scroll
                .bounds()
        })
    }

    fn offset_y(&mut self) -> Pixels {
        in_window(&self.app, &mut self.settings, |app, _, _| {
            app.active_settings()
                .expect("settings open")
                .content_scroll
                .offset()
                .y
        })
    }

    fn recording(&mut self) -> Option<String> {
        in_window(&self.app, &mut self.settings, |app, _, _| {
            app.active_settings()
                .and_then(|s| s.recording.as_ref())
                .map(|r| r.action.clone())
        })
    }

    /// The action whose keyboard ring is mounted in the settings window now.
    fn focused_cell(&mut self) -> Option<String> {
        let settings = &mut self.settings;
        self.cells
            .iter()
            .find(|c| settings.debug_bounds(c.ring).is_some())
            .map(|c| c.action.clone())
    }

    /// Real Tabs, each followed by `run_until_parked` only, until the target's
    /// ring is mounted.
    fn tab_to_target(&mut self) -> bool {
        let action = self.cells[self.target].action.clone();
        for _ in 0..MAX_TABS {
            if self.focused_cell().as_deref() == Some(action.as_str()) {
                return true;
            }
            self.settings.simulate_keystrokes("tab");
            self.settings.run_until_parked();
        }
        self.focused_cell().as_deref() == Some(action.as_str())
    }
}

/// Open the settings window through `open_settings_window_then`, on the
/// Keyboard Shortcuts page, ACTIVATE it, and prove before any claim a test makes
/// that it is a second window holding the page, at its top, with a cell that
/// starts entirely below the fold.
fn open_real(cx: &mut TestAppContext) -> Real {
    let (app, mut workspace) = harness(cx);
    let owner_handle = workspace.update(|window, _| window.window_handle());
    assert!(
        in_window(&app, &mut workspace, |app, _, _| app
            .settings_window
            .is_none()
            && !app.has_settings()),
        "setup: settings is closed before it opens"
    );
    in_window(&app, &mut workspace, |app, window, cx| {
        app.open_settings_window_then(
            window,
            cx,
            Some(Box::new(|this, window, cx| {
                this.navigate_settings(SettingsSection::Keybindings, None, window, cx)
            })),
        );
    });
    workspace.run_until_parked();

    let (settings_handle, _) = in_window(&app, &mut workspace, |app, _, _| app.settings_window)
        .expect("setup: opening records the settings window");
    assert_ne!(
        settings_handle, owner_handle,
        "setup: the settings window is not the workspace window"
    );
    assert_eq!(cx.windows().len(), 2, "setup: a second window is open");
    let is_settings_window = settings_handle
        .downcast::<Root>()
        .expect("setup: the settings window is a Root")
        .update(cx, |root, _, _| {
            root.view()
                .clone()
                .downcast::<crate::ui::settings_window::SettingsWindow>()
                .is_ok()
        })
        .expect("setup: the settings window is alive");
    assert!(
        is_settings_window,
        "setup: the second window's root view is a SettingsWindow"
    );

    let mut settings = VisualTestContext::from_window(settings_handle, cx);
    settings.update(|window, _| window.activate_window());
    settings.simulate_resize(size(px(NARROW.0), px(NARROW.1)));
    settle(&mut settings);
    settle(&mut workspace);
    assert!(
        settings.update(|window, _| window.is_window_active()),
        "setup: the settings window is the active window"
    );
    assert!(
        in_window(&app, &mut settings, |app, _, _| app
            .active_settings()
            .map(|s| s.section))
            == Some(SettingsSection::Keybindings),
        "setup: the follow-up ran against the new window's state (the shortcuts page)"
    );

    let actions: Vec<String> = settings.update(|_, cx| {
        crate::ui::keymap::effective_chords(cx)
            .into_iter()
            .map(|(action, _)| action)
            .collect()
    });
    let cells: Vec<Cell> = actions
        .into_iter()
        .map(|action| {
            let cell: &'static str = Box::leak(format!("kb-{action}").into_boxed_str());
            let ring: &'static str = Box::leak(format!("kb-ring-{action}").into_boxed_str());
            Cell { action, cell, ring }
        })
        .collect();

    let mut real = Real {
        app,
        _workspace: workspace,
        settings,
        cells,
        target: 0,
    };
    let view = real.viewport();
    assert!(view.size.height > px(0.), "setup: the page is laid out");
    assert_eq!(real.offset_y(), px(0.), "setup: the page is at its top");
    let settings = &mut real.settings;
    let target = real
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, c)| settings.debug_bounds(c.cell).map(|b| (i, b)))
        .filter(|(_, b)| b.top() >= view.bottom())
        .min_by(|a, b| {
            f32::from(a.1.top())
                .partial_cmp(&f32::from(b.1.top()))
                .expect("finite")
        })
        .map(|(i, _)| i)
        .expect("setup: some cell starts entirely below the viewport");
    real.target = target;
    let b = real
        .settings
        .debug_bounds(real.cells[target].cell)
        .expect("drawn");
    assert!(
        !within(b, view) && b.top() >= view.bottom(),
        "setup, before any focus moves: {} starts below the fold ({b:?} vs {view:?})",
        real.cells[target].action
    );
    assert!(
        real._workspace
            .debug_bounds(real.cells[target].cell)
            .is_none(),
        "setup: the cell is not drawn in the workspace window"
    );

    let handle = in_window(&real.app, &mut real.settings, |app, window, cx| {
        let search = app
            .active_settings()
            .expect("settings open")
            .shortcut_search
            .clone();
        search.update(cx, |s, cx| s.focus(window, cx));
        search.read(cx).focus_handle(cx)
    });
    real.settings.run_until_parked();
    assert!(
        real.settings.update(|window, _| handle.is_focused(window)),
        "setup: the shortcut filter holds focus in the settings window"
    );
    real
}

/// r1: in the real settings window a real Tab to a cell below the fold reveals the
/// cell and its ring (no explicit draw after the key), and Enter on it records for
/// it without moving the page.
#[gpui::test]
fn r1_a_real_tab_reveals_a_below_fold_cell_in_the_settings_window(cx: &mut TestAppContext) {
    let mut real = open_real(cx);
    let action = real.cells[real.target].action.clone();
    assert!(
        real.tab_to_target(),
        "FOCUS: Tab never reached the {action} cell in the settings window"
    );
    let view = real.viewport();
    let at = real.offset_y();
    assert!(
        at < px(0.),
        "the settings page did not scroll to a cell that began below the fold"
    );
    let cell = real
        .settings
        .debug_bounds(real.cells[real.target].cell)
        .expect("drawn");
    let ring = real
        .settings
        .debug_bounds(real.cells[real.target].ring)
        .expect("mounted");
    assert!(
        within(cell, view),
        "the focused cell {cell:?} is not inside the viewport {view:?}"
    );
    assert!(
        within(ring, view),
        "its keyboard ring {ring:?} is not inside the viewport {view:?}"
    );

    press(&mut real.settings, "enter");
    assert_eq!(
        real.recording().as_deref(),
        Some(action.as_str()),
        "Enter on the revealed cell records for it"
    );
    assert_eq!(real.offset_y(), at, "starting the recording moved the page");
    press(&mut real.settings, "escape");
    assert_eq!(real.recording(), None, "Escape cancels the recording");
    assert_eq!(
        real.offset_y(),
        at,
        "cancelling the recording moved the page"
    );
}

/// r2: unmount ownership in the real window. Leaving the page with the revealed
/// cell focused releases its handle and nothing scrolls the page afterwards. Only
/// a weak handle leaves the closure that reads the focus.
#[gpui::test]
fn r2_leaving_the_page_releases_the_focused_cell_in_the_settings_window(cx: &mut TestAppContext) {
    let mut real = open_real(cx);
    let action = real.cells[real.target].action.clone();
    assert!(
        real.tab_to_target(),
        "FOCUS: Tab never reached the {action} cell in the settings window"
    );
    let weak = real
        .settings
        .update(|window, cx| window.focused(cx).map(|handle| handle.downgrade()))
        .expect("setup: a cell holds focus");
    assert!(
        weak.upgrade().is_some(),
        "setup: the cell's handle is alive with no strong reference in the test"
    );

    in_window(&real.app, &mut real.settings, |app, window, cx| {
        app.navigate_settings(SettingsSection::Appearance, None, window, cx);
    });
    settle(&mut real.settings);
    assert!(
        real.settings
            .debug_bounds(real.cells[real.target].cell)
            .is_none(),
        "setup: the cell is no longer drawn in the settings window"
    );
    assert!(
        weak.upgrade().is_none(),
        "the unmounted cell's focus handle was released"
    );
    let after = real.offset_y();
    settle(&mut real.settings);
    settle(&mut real.settings);
    assert_eq!(
        real.offset_y(),
        after,
        "something scrolled the settings page after the cell was gone"
    );
}

// ------------------------------------------------------------------ the Reset link
//
// The self-removing Reset link of a modified row, in the real settings window.
// `reset-NewTab` is the stop key the link is expected to carry once it is a
// shared Stop, so the real-Tab test is RED until then; the click baseline passes
// on today's tree.

fn marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|w, _| {
        w.context_stack().iter().rev().find_map(|c| {
            c.get(crate::ui::settings::kit::STOP_MARK)
                .map(|v| v.to_string())
        })
    })
}

fn focused(vcx: &mut VisualTestContext) -> Option<gpui::FocusHandle> {
    vcx.update(|w, cx| w.focused(cx))
}

fn new_tab_is_overridden(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|_, cx| {
        cx.global::<crate::core::config::Config>()
            .keybindings
            .contains_key("NewTab")
    })
}

/// Real Tabs (`run_until_parked` after each) until the marker is `id`, or the
/// focus stops moving to new handles.
fn tab_to_marker(vcx: &mut VisualTestContext, id: &str) -> bool {
    let mut seen = vec![focused(vcx)];
    for _ in 0..MAX_TABS {
        if marker(vcx).as_deref() == Some(id) {
            return true;
        }
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        let now = focused(vcx);
        if now.is_none() || seen.contains(&now) {
            return marker(vcx).as_deref() == Some(id);
        }
        seen.push(now);
    }
    marker(vcx).as_deref() == Some(id)
}

/// The settings window, opened through `open_settings_window_then` on the
/// Keyboard Shortcuts page, ACTIVATED, tall enough that every row is on screen,
/// with New Tab carrying one override (a LABELLED CONFIG SETUP, not a UI path)
/// and the shortcut filter focused. Returns the app, the workspace context (kept
/// alive) and the settings window's context.
fn open_reset_window(
    cx: &mut TestAppContext,
) -> (Entity<Tty59App>, VisualTestContext, VisualTestContext) {
    use crate::core::config::{Config, KeybindingOverride};
    let (app, mut workspace) = harness(cx);
    let owner_handle = workspace.update(|window, _| window.window_handle());
    in_window(&app, &mut workspace, |app, window, cx| {
        app.open_settings_window_then(
            window,
            cx,
            Some(Box::new(|this, window, cx| {
                this.navigate_settings(SettingsSection::Keybindings, None, window, cx)
            })),
        );
    });
    workspace.run_until_parked();
    let (settings_handle, _) = in_window(&app, &mut workspace, |app, _, _| app.settings_window)
        .expect("setup: opening records the settings window");
    assert_ne!(
        settings_handle, owner_handle,
        "setup: the settings window is not the workspace window"
    );
    assert_eq!(cx.windows().len(), 2, "setup: a second window is open");

    let mut settings = VisualTestContext::from_window(settings_handle, cx);
    settings.update(|window, _| window.activate_window());
    settings.simulate_resize(size(px(1100.), px(4000.)));
    settle(&mut settings);
    settle(&mut workspace);
    assert!(
        settings.update(|window, _| window.is_window_active()),
        "setup: the settings window is the active window"
    );

    // SETUP (config): one override on New Tab, then a re-render of that window.
    settings.update(|_, cx| {
        cx.global_mut::<Config>().keybindings.insert(
            "NewTab".to_string(),
            KeybindingOverride::Add("ctrl-alt-shift-n".to_string()),
        );
        crate::ui::keymap::rebind(cx);
    });
    in_window(&app, &mut settings, |_, _, cx| cx.notify());
    settle(&mut settings);
    assert!(
        new_tab_is_overridden(&mut settings),
        "setup: New Tab carries an override"
    );
    assert!(
        settings.debug_bounds("reset-NewTab").is_some(),
        "setup: the modified row draws its Reset link in the settings window"
    );
    assert!(
        workspace.debug_bounds("reset-NewTab").is_none(),
        "setup: and not in the workspace window"
    );

    let handle = in_window(&app, &mut settings, |app, window, cx| {
        let search = app
            .active_settings()
            .expect("settings open")
            .shortcut_search
            .clone();
        search.update(cx, |s, cx| s.focus(window, cx));
        search.read(cx).focus_handle(cx)
    });
    settings.run_until_parked();
    assert!(
        settings.update(|window, _| handle.is_focused(window)),
        "setup: the shortcut filter holds focus in the settings window"
    );
    (app, workspace, settings)
}

/// r3: the baseline in the real window. A pointer click on the Reset link removes
/// the override and the link, and leaves the row's cell. Passes on today's tree.
#[gpui::test]
fn r3_a_click_on_reset_removes_the_override_and_the_link_in_the_settings_window(
    cx: &mut TestAppContext,
) {
    let (_app, _workspace, mut settings) = open_reset_window(cx);
    let at = settings.debug_bounds("reset-NewTab").expect("drawn");
    settings.simulate_click(at.center(), gpui::Modifiers::none());
    settle(&mut settings);
    assert!(
        !new_tab_is_overridden(&mut settings),
        "Reset removed New Tab's override"
    );
    assert!(
        settings.debug_bounds("reset-NewTab").is_none(),
        "the row is no longer modified, so its Reset link is gone"
    );
    assert!(
        settings.debug_bounds("kb-NewTab").is_some(),
        "the row's shortcut cell stays"
    );
}

/// r4: the keyboard path in the real window. A real Tab reaches the Reset link
/// (marker `reset-NewTab`), the handle is held weakly only, Enter resets, the link
/// and its handle go away, a real Tab lands on something mounted that holds
/// focus, and ONE Escape closes the real window.
#[gpui::test]
fn r4_tab_to_reset_then_enter_removes_the_link_and_the_window_keyboard_survives(
    cx: &mut TestAppContext,
) {
    let (app, mut workspace, mut settings) = open_reset_window(cx);
    assert!(
        tab_to_marker(&mut settings, "reset-NewTab"),
        "FOCUS: Tab never reached the Reset link (marker reset-NewTab) in the settings window"
    );
    let weak = settings
        .update(|window, cx| window.focused(cx).map(|handle| handle.downgrade()))
        .expect("setup: the Reset link holds focus");
    assert!(
        weak.upgrade().is_some(),
        "setup: the link's handle is alive with no strong reference in the test"
    );

    press(&mut settings, "enter");
    assert!(
        !new_tab_is_overridden(&mut settings),
        "Enter on the focused Reset link removed New Tab's override"
    );
    assert!(
        settings.debug_bounds("reset-NewTab").is_none(),
        "the Reset link left the page"
    );
    settle(&mut settings);
    assert!(
        weak.upgrade().is_none(),
        "the removed link's focus handle was released"
    );

    press(&mut settings, "tab");
    let (mounted, stack) = settings.update(|window, _| {
        let stack = window.context_stack();
        (
            stack.iter().any(|c| c.contains("Root")),
            format!("{stack:?}"),
        )
    });
    assert!(
        mounted,
        "after Tab the window's focus is on nothing drawn; stack: {stack}"
    );
    let live_new_handle = settings.update(|window, cx| {
        window
            .focused(cx)
            .is_some_and(|focused| weak != focused && focused.is_focused(window))
    });
    assert!(
        live_new_handle,
        "after Tab the focus is not a new handle that reports itself focused"
    );

    // One Escape closes the real window; its key-up has no destination there.
    settings.simulate_keystrokes("escape");
    workspace.run_until_parked();
    let (window, state) = in_window(&app, &mut workspace, |app, _, _| {
        (app.settings_window.is_some(), app.has_settings())
    });
    assert!(
        !window && !state,
        "one Escape did not close the settings window after the Reset link was removed \
         (window recorded: {window}, state held: {state})"
    );
}
