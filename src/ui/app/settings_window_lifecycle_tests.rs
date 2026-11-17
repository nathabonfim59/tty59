//! A2 reviewer tests: the settings page in the real, separate `SettingsWindow`.
//!
//! Every other settings test runs the page inline, because `toggle_settings`
//! and `open_settings_then` build the state in the workspace window under
//! `cfg!(test)`. These go through `open_settings_window_then` — the function
//! the app itself calls — so the page is drawn, focused and closed in a second
//! window, the way a person meets it. This module is a child of `app` only so
//! it can reach that private function.
//!
//! What is pinned down:
//! * a shortcut cell owns its focus handle: while mounted the page keeps it
//!   alive without any help from the test, and once Back (or anything else)
//!   unmounts the cell the handle is released — nothing keeps a stale one;
//! * with the cell's focus gone, the window's keyboard still works: Escape
//!   still reaches the page and closes the window, Tab still lands on
//!   something that really holds focus;
//! * Reset on a cell that stays mounted does not move or drop its focus.
//!
//! How the handle is watched: the test never holds a strong handle across the
//! unmount. It takes the live handle from `window.focused`, downgrades it
//! inside the same closure (so the strong clone is gone before the closure
//! returns) and keeps only the `WeakFocusHandle`. A test that kept the strong
//! handle would keep the cell's focus alive itself and report a window whose
//! focus never left — the fake focus these tests exist to rule out.
//!
//! Honest limits:
//! * The Back link (`settings-back`) and the Reset link (`reset-{action}`)
//!   carry no `debug_selector`, so they cannot be clicked from here. Back runs
//!   the body of its click handler, `navigate_settings(KeyboardMouse)`; Reset
//!   runs `reset_keybinding`. That the links call those is not tested here.
//! * Whether the Reset link has itself left the page is not observable without
//!   a selector, so nothing here claims it.
//! * `modified_only` does not filter the Keybindings page, so a row cannot be
//!   removed by Reset; the only way a focused cell unmounts on that page is
//!   navigating away, which is what the Back tests do.

use super::Tty59App;
use crate::ui::settings::SettingsSection;
use crate::ui::settings::kit::test_support::harness;
use gpui::{
    AnyWindowHandle, Entity, Focusable as _, KeyUpEvent, Keystroke, Modifiers, TestAppContext,
    VisualTestContext, WeakFocusHandle, px, size,
};
use gpui_component::Root;

/// A Tty59App is shared by the workspace and SettingsWindow. Entity::update_in
/// follows GPUI's registered window for the entity, which can be either one
/// after their renders. Enter the requested test window explicitly instead.
fn in_window<R>(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    f: impl FnOnce(&mut Tty59App, &mut gpui::Window, &mut gpui::Context<Tty59App>) -> R,
) -> R {
    vcx.update(|window, cx| app.update(cx, |app, cx| f(app, window, cx)))
}

/// Two paints: the first lays the page out, the second reads the bounds and
/// focus that layout produced.
fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..2 {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
}

/// Down, then up while the target window still exists. A key-down that closes
/// the window has no key-up destination in that closed window.
fn press(vcx: &mut VisualTestContext, key: &str) {
    let target = vcx.update(|window, _| window.window_handle());
    vcx.simulate_keystrokes(key);
    if vcx.windows().contains(&target) {
        vcx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("a key name"),
        });
    }
    vcx.run_until_parked();
}

fn recording(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Option<(String, Vec<String>)> {
    in_window(app, vcx, |app, _, _| {
        app.active_settings()
            .and_then(|s| s.recording.as_ref())
            .map(|r| (r.action.clone(), r.chords.clone()))
    })
}

fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    let at = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    vcx.simulate_click(at.center(), Modifiers::none());
    vcx.run_until_parked();
}

/// The workspace window and the settings window, opened the way the app opens
/// them, with the Keyboard Shortcuts page showing in the second.
struct Real {
    cx: TestAppContext,
    app: Entity<Tty59App>,
    workspace: VisualTestContext,
    settings: VisualTestContext,
    settings_handle: AnyWindowHandle,
    owner_handle: AnyWindowHandle,
}

/// Open the settings window through `open_settings_window_then`, with a
/// follow-up that loads the shortcuts page, and prove — before any claim a
/// test makes — that what is open is a second window holding the page.
fn open_real(cx: &mut TestAppContext) -> Real {
    let (app, mut workspace) = harness(cx);
    let owner_handle = workspace.update(|window, _| window.window_handle());
    assert_eq!(
        cx.windows().len(),
        1,
        "setup: the workspace is the only window before settings opens"
    );
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

    let (settings_handle, owner) = in_window(&app, &mut workspace, |app, _, _| app.settings_window)
        .expect("setup: opening records the settings window");
    assert_eq!(owner, owner_handle, "setup: the owner is the workspace");
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
    settings.simulate_resize(size(px(1100.), px(4000.)));
    settle(&mut settings);
    settle(&mut workspace);

    // `SettingsSection` has no `Debug`, so this is an `assert!` on `==`.
    assert!(
        in_window(&app, &mut settings, |app, _, _| app
            .active_settings()
            .map(|s| s.section))
            == Some(SettingsSection::Keybindings),
        "setup: the follow-up ran against the new window's state (the shortcuts page)"
    );
    assert!(
        settings.debug_bounds("kb-NewTab").is_some(),
        "setup: the page's own cell is drawn in the settings window"
    );
    assert!(
        workspace.debug_bounds("kb-NewTab").is_none(),
        "setup: and not in the workspace window"
    );
    let initial_focus_is_the_search_field = in_window(&app, &mut settings, |app, window, cx| {
        let search = app
            .active_settings()
            .expect("setup: the page has state")
            .search
            .read(cx)
            .focus_handle(cx);
        window.focused(cx) == Some(search)
    });
    assert!(
        initial_focus_is_the_search_field,
        "setup: the settings window opens focused on its search field"
    );
    let workspace_holds_page_focus = in_window(&app, &mut workspace, |app, window, cx| {
        let page = app.active_settings().map(|s| s.focus_handle.clone());
        let search = app
            .active_settings()
            .map(|s| s.search.read(cx).focus_handle(cx));
        let focused = window.focused(cx);
        focused.is_some() && (focused == page || focused == search)
    });
    assert!(
        !workspace_holds_page_focus,
        "setup: the workspace window does not hold the page's focus"
    );

    Real {
        cx: cx.clone(),
        app,
        workspace,
        settings,
        settings_handle,
        owner_handle,
    }
}

impl Real {
    /// Focus New Tab's cell the way the foundation's own tests prove it: a
    /// click records, Escape cancels and leaves focus on the cell, Enter
    /// records again — which only the focused cell can do — and Escape
    /// cancels. Returns only a weak handle to it, after proving it is the
    /// cell's and that nothing else but the page keeps it alive.
    fn focus_new_tab_cell(&mut self) -> WeakFocusHandle {
        click(&mut self.settings, "kb-NewTab");
        assert_eq!(
            recording(&self.app, &mut self.settings),
            Some(("NewTab".to_string(), Vec::new())),
            "setup: the click records for New Tab"
        );
        press(&mut self.settings, "escape");
        settle(&mut self.settings);
        assert_eq!(
            recording(&self.app, &mut self.settings),
            None,
            "setup: Escape cancels the recording"
        );
        assert!(
            self.settings_window_is_open(),
            "setup: Escape while recording cancels the recording, not the window"
        );
        press(&mut self.settings, "enter");
        settle(&mut self.settings);
        assert_eq!(
            recording(&self.app, &mut self.settings),
            Some(("NewTab".to_string(), Vec::new())),
            "setup: Enter records for New Tab, so its cell holds focus"
        );
        press(&mut self.settings, "escape");
        settle(&mut self.settings);
        assert_eq!(
            recording(&self.app, &mut self.settings),
            None,
            "setup: Escape cancels again"
        );
        assert!(
            self.settings_window_is_open(),
            "setup: the window is still open with a cell focused"
        );

        // The strong handle never leaves this closure.
        let weak = in_window(&self.app, &mut self.settings, |app, window, cx| {
            let state = app.active_settings().expect("setup: the page has state");
            let focused = window.focused(cx).expect("setup: a cell holds focus");
            assert!(
                focused != state.focus_handle && focused != state.search.read(cx).focus_handle(cx),
                "setup: the focused handle is neither the page's root nor its search field"
            );
            assert!(
                focused.is_focused(window),
                "setup: the handle reports itself focused"
            );
            focused.downgrade()
        });
        assert!(
            weak.upgrade().is_some(),
            "setup: the cell's handle is alive with no strong reference in the test: \
             the page owns it"
        );
        // The fixture positive for the mount witness the unmount tests lean on:
        // a cell that is really focused and drawn must show `Root` in the
        // context stack. If this fails, `focus_is_mounted` is not a valid
        // witness on this gpui / gpui-component pin and its absence after Back
        // proves nothing.
        let (mounted, stack) = self.focus_context_report();
        assert!(
            mounted,
            "setup: the focused, drawn cell's context stack has no Root, so the mount \
             witness is invalid here; stack: {stack}"
        );
        weak
    }

    fn settings_window_is_open(&mut self) -> bool {
        let (window, state) = in_window(&self.app, &mut self.workspace, |app, _, _| {
            (app.settings_window.is_some(), app.has_settings())
        });
        window && state
    }

    fn focused_is_alive(&mut self) -> Option<bool> {
        self.settings
            .update(|window, cx| window.focused(cx).map(|handle| handle.is_focused(window)))
    }

    /// Whether the keyboard focus sits on an element that is drawn. The
    /// context stack is the dispatch path from the window's root to the focused
    /// element in the frame on screen: when the focused id is not in that frame
    /// (its element is gone, or the handle is dead) gpui falls back to the bare
    /// root node, whose path carries no key context at all, and `Root`'s own
    /// context only appears for a focus that is really mounted. `is_focused`
    /// alone says nothing about this: it is true of an id that left the tree.
    fn focus_is_mounted(&mut self) -> bool {
        self.focus_context_report().0
    }

    /// `focus_is_mounted`, with the whole context stack as text for a failure
    /// message.
    fn focus_context_report(&mut self) -> (bool, String) {
        self.settings.update(|window, _| {
            let stack = window.context_stack();
            let mounted = stack.iter().any(|context| context.contains("Root"));
            (mounted, format!("{stack:?}"))
        })
    }

    /// Back, as its click handler does it.
    fn back(&mut self) {
        in_window(&self.app, &mut self.settings, |app, window, cx| {
            app.navigate_settings(SettingsSection::KeyboardMouse, None, window, cx)
        });
        settle(&mut self.settings);
        // Released within a couple of frames, not "eventually".
        settle(&mut self.settings);
        assert!(
            in_window(&self.app, &mut self.settings, |app, _, _| app
                .active_settings()
                .map(|s| s.section))
                == Some(SettingsSection::KeyboardMouse),
            "setup: Back left the shortcuts page"
        );
        assert!(
            self.settings.debug_bounds("kb-NewTab").is_none(),
            "setup: the cell is no longer drawn"
        );
    }

    /// Everything that has to be true once the window has been closed.
    fn assert_closed(&mut self, why: &str) {
        let (window, state) = in_window(&self.app, &mut self.workspace, |app, _, _| {
            (app.settings_window.is_some(), app.has_settings())
        });
        assert!(!window, "{why}: the app still records a settings window");
        assert!(!state, "{why}: the app still holds settings state");
        assert_eq!(
            self.cx.windows().len(),
            1,
            "{why}: the settings window is still open"
        );
        let gone = self
            .cx
            .update(|cx| self.settings_handle.update(cx, |_, _, _| ()).is_err());
        assert!(gone, "{why}: the settings window handle still answers");
        assert!(
            self.workspace
                .update(|window, cx| window.focused(cx))
                .is_some(),
            "{why}: the workspace window did not get the keyboard back"
        );
        assert_eq!(
            self.owner_handle,
            self.workspace.update(|window, _| window.window_handle()),
            "{why}: the workspace is still the owner"
        );
    }

    /// Escape, once, in the settings window.
    fn escape(&mut self) {
        press(&mut self.settings, "escape");
        self.workspace.run_until_parked();
    }
}

/// S0: the setup itself. Opening through the app's own function gives a second
/// window whose root is a `SettingsWindow`, holding the page and the
/// page-owned cell, with the workspace window not holding the page's focus.
/// All of that is asserted inside `open_real`; this test is that setup alone,
/// so a failure there is named here and not blamed on a lifecycle claim.
#[gpui::test]
fn the_page_opens_in_a_real_second_window(cx: &mut TestAppContext) {
    let mut real = open_real(cx);
    assert!(real.settings_window_is_open());
}

/// T5, the control the others are measured against: with a cell focused and
/// mounted, Escape reaches the page from it and closes the real window,
/// returning the keyboard to the workspace.
#[gpui::test]
fn escape_from_a_focused_cell_closes_the_real_window(cx: &mut TestAppContext) {
    let mut real = open_real(cx);
    real.focus_new_tab_cell();
    assert_eq!(
        real.focused_is_alive(),
        Some(true),
        "setup: the cell really holds focus before Escape"
    );
    real.escape();
    real.assert_closed("Escape from a focused cell");
}

/// T1: Back unmounts the focused cell. The keyboard symptom comes first: a
/// fresh Keyboard & Mouse page with no query, menu, conflict or recording has
/// nothing for Escape to peel, so ONE real Escape must close the window,
/// whatever became of the cell's handle. Only after that does the test report
/// the ownership diagnostic, read BEFORE the key (`weak_dead`) so it cannot be
/// changed by the window having been dropped: the handle should be released,
/// the page keeping no stale owner.
#[gpui::test]
fn back_releases_the_cells_focus_and_escape_still_closes(cx: &mut TestAppContext) {
    let mut real = open_real(cx);
    let weak = real.focus_new_tab_cell();

    real.back();
    let weak_dead = weak.upgrade().is_none();
    let mounted = real.focus_is_mounted();

    real.escape();
    assert!(
        !real.settings_window_is_open(),
        "one Escape did nothing after Back unmounted the focused cell: the window's \
         keyboard is dead (cell handle released before the key: {weak_dead}, focus on \
         a mounted element before the key: {mounted})"
    );
    real.assert_closed("Escape after Back");

    assert!(
        weak_dead,
        "the unmounted cell's focus handle was still alive when Back finished: \
         something other than the cell keeps it, so the window reports focus on a \
         cell that is not drawn (focus on a mounted element before the key: {mounted})"
    );
}

/// T2: the same unmount, then a real Tab. Focus must land on an element that
/// is MOUNTED (`context_stack` carries `Root`, which `is_focused` cannot tell:
/// it is true of ids that left the tree) and is not the old cell's handle, and
/// then ONE Escape must close the window. The ownership diagnostic
/// (`weak_dead`) is captured before the keys and asserted last.
#[gpui::test]
fn back_then_tab_lands_on_something_that_holds_focus(cx: &mut TestAppContext) {
    let mut real = open_real(cx);
    let weak = real.focus_new_tab_cell();
    real.back();
    let weak_dead = weak.upgrade().is_none();

    press(&mut real.settings, "tab");
    settle(&mut real.settings);

    let (mounted, stack) = real.focus_context_report();
    let new_live_handle = real.settings.update(|window, cx| {
        window
            .focused(cx)
            .is_some_and(|focused| weak != focused && focused.is_focused(window))
    });
    assert!(
        mounted,
        "after Tab the window's focus is on nothing drawn: its context stack has no \
         Root (cell handle released before the keys: {weak_dead}); stack: {stack}"
    );
    assert!(
        new_live_handle,
        "after Tab the window's focus is not a new handle that reports itself focused: \
         it is none, or still the old cell's (cell handle released before the keys: \
         {weak_dead})"
    );

    real.escape();
    assert!(
        !real.settings_window_is_open(),
        "one Escape did nothing after Back and Tab (cell handle released before the \
         keys: {weak_dead})"
    );
    real.assert_closed("Escape after Back and Tab");

    assert!(
        weak_dead,
        "the unmounted cell's focus handle was still alive when Back finished: \
         something other than the cell keeps it"
    );
}

/// T4: Reset on a cell that stays mounted. The cell keeps the very handle it
/// had, still holds focus, and still records through the keyboard — Reset
/// removing its own link must not take the focus with it. Reset runs as the
/// body of its click handler, `reset_keybinding`; see the module notes.
#[gpui::test]
fn reset_leaves_a_mounted_cells_focus_alone(cx: &mut TestAppContext) {
    let mut real = open_real(cx);
    // Unassigned the way a person does it, so Reset has something to undo.
    in_window(&real.app, &mut real.settings, |app, window, cx| {
        app.start_recording_key("NewTab".to_string(), window, cx)
    });
    real.settings.simulate_keystrokes("backspace");
    settle(&mut real.settings);
    let was_default = real
        .settings
        .update(|_, cx| crate::ui::keymap::effective_key("NewTab", cx))
        .is_some();
    assert!(
        !was_default,
        "setup: Backspace left New Tab unassigned, so it is modified"
    );

    let weak = real.focus_new_tab_cell();

    in_window(&real.app, &mut real.settings, |app, _, cx| {
        app.reset_keybinding("NewTab".to_string(), cx)
    });
    settle(&mut real.settings);

    let restored = real
        .settings
        .update(|_, cx| crate::ui::keymap::effective_key("NewTab", cx))
        .is_some();
    assert!(restored, "setup: Reset gave New Tab its default chord back");
    assert!(
        real.settings.debug_bounds("kb-NewTab").is_some(),
        "setup: the cell is still drawn after Reset"
    );
    let same = real.settings.update(|window, cx| {
        window
            .focused(cx)
            .is_some_and(|focused| weak == focused && focused.is_focused(window))
    });
    assert!(
        same,
        "after Reset the window's focus is the same cell handle, still focused"
    );
    assert_eq!(
        recording(&real.app, &mut real.settings),
        None,
        "Reset left nothing recording"
    );

    press(&mut real.settings, "enter");
    settle(&mut real.settings);
    assert_eq!(
        recording(&real.app, &mut real.settings).map(|(action, _)| action),
        Some("NewTab".to_string()),
        "Enter still records for New Tab after Reset: the cell kept its focus"
    );
    press(&mut real.settings, "escape");
    settle(&mut real.settings);
    real.escape();
    real.assert_closed("Escape after Reset");
}
