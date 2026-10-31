use gpui::{App, Context, ModifiersChangedEvent, Window};

use crate::ui::app::Tty59App;

const BADGE_DELAY_MS: u64 = 200;

/// Only `1`…`9` have activation chords, so badges stop at nine.
pub(crate) const MAX_TAB_BADGES: usize = 9;

pub(crate) fn tab_badge_label(index: usize) -> String {
    (index + 1).to_string()
}

pub(crate) fn badge_positions_from_order(
    tab_count: usize,
    order: impl IntoIterator<Item = usize>,
) -> Vec<Option<usize>> {
    let mut positions = vec![None; tab_count];
    for (n, i) in order.into_iter().take(MAX_TAB_BADGES).enumerate() {
        if let Some(position) = positions.get_mut(i) {
            *position = Some(n);
        }
    }
    positions
}

impl Tty59App {
    pub(crate) fn on_modifiers_changed(
        &mut self,
        ev: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = &ev.modifiers;
        self.switcher_hold_changed(m, window, cx);
        self.set_link_modifier(m.secondary(), cx);

        // The numbers drawn on the tabs are `ActivateTab1`…`ActivateTab9`, so
        // the hold that raises them is the one those chords are on — Alt off
        // macOS, ⌘ on it, or wherever a rebind moved them. The enabled group
        // family has its own hold and takes precedence if both families were
        // deliberately given the same modifiers.
        let group_hold = crate::ui::keymap::group_tab_badge_hold(cx);
        let all_hold = crate::ui::keymap::tab_badge_hold(cx);
        let group = if group_hold.as_ref() == Some(m) {
            Some(true)
        } else if all_hold.as_ref() == Some(m) {
            Some(false)
        } else {
            None
        };

        self.mod_hint_gen = self.mod_hint_gen.wrapping_add(1);
        let Some(group) = group else {
            self.dismiss_mod_hint(cx);
            return;
        };
        self.mod_hint_group = group;

        let generation = self.mod_hint_gen;
        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(BADGE_DELAY_MS)).await;
            let _ = this.update(cx, |this, cx| {
                if this.mod_hint_gen == generation {
                    this.mod_hint_badges = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Badge positions by tab index, or `None` while no hold is showing them.
    /// Every surface reads this same mapping as the activation actions:
    /// - With `tab_group_shortcuts` off, holding the tab modifier numbers all
    ///   tabs in visual order.
    /// - With `tab_group_shortcuts` on, holding the tab modifier numbers only
    ///   the active group's tabs (starting at one), while holding the group
    ///   modifier numbers each group's first tab (used by the top strip and any
    ///   headerless sidebar section, and read by sidebar group headers).
    pub(crate) fn tab_badge_positions(&self, cx: &App) -> Option<Vec<Option<usize>>> {
        let cfg = cx.global::<crate::core::config::Config>();
        let group_shortcuts = cfg.tab_group_shortcuts;
        if !self.mod_hint_badges || (self.mod_hint_group && !group_shortcuts) {
            return None;
        }
        let positions = if self.mod_hint_group {
            badge_positions_from_order(
                self.tabs.len(),
                self.group_tab_orders(cx)
                    .into_iter()
                    .filter_map(|tabs| tabs.first().copied()),
            )
        } else if group_shortcuts {
            badge_positions_from_order(self.tabs.len(), self.active_group_tab_order(cx))
        } else if cfg.tab_bar_position == crate::core::config::TabBarPosition::Left {
            badge_positions_from_order(self.tabs.len(), self.visual_tab_order(cx))
        } else {
            badge_positions_from_order(self.tabs.len(), 0..self.tabs.len())
        };
        Some(positions)
    }

    pub(crate) fn set_link_modifier(&mut self, down: bool, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            for leaf in tab.pane.terminals() {
                leaf.update(cx, |view, cx| {
                    view.refresh_link_hover(down, cx);
                });
            }
        }
    }

    pub(crate) fn dismiss_mod_hint(&mut self, cx: &mut Context<Self>) {
        self.mod_hint_gen = self.mod_hint_gen.wrapping_add(1);
        self.mod_hint_group = false;
        if self.mod_hint_badges {
            self.mod_hint_badges = false;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_badge_label_is_the_bare_digit() {
        assert_eq!(tab_badge_label(0), "1");
        assert_eq!(tab_badge_label(8), "9");
    }

    #[test]
    fn badge_positions_stop_at_nine() {
        let positions = badge_positions_from_order(12, 0..12);
        assert_eq!(positions[8], Some(8));
        assert_eq!(positions[9], None);
        assert_eq!(positions[11], None);
    }
}

#[cfg(test)]
mod gpui_tests {
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::app::Tty59App;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, WindowHandle};

    fn harness(cx: &mut TestAppContext) -> (WindowHandle<Tty59App>, VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
        });
        let window = cx.add_window(|window, cx| {
            Tty59App::with_session(None, Some(Session::default()), window, cx)
        });
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.background_executor.run_until_parked();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (window, vcx)
    }

    fn badges_shown(window: &WindowHandle<Tty59App>, cx: &mut TestAppContext) -> bool {
        window
            .update(cx, |app, _, _| app.mod_hint_badges)
            .expect("the app window stays open")
    }

    /// The hold the badges answer to, read from the same place they do rather
    /// than assumed — the shipped one is ⌘ on macOS and Alt off it.
    fn hold_modifiers(window: &WindowHandle<Tty59App>, cx: &mut TestAppContext) -> Modifiers {
        window
            .update(cx, |_, _, cx| crate::ui::keymap::tab_badge_hold(cx))
            .expect("the app window stays open")
            .expect("the shipped tab chords are on a modifier")
    }

    fn wait_for_badges(window: &WindowHandle<Tty59App>, cx: &mut TestAppContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            cx.background_executor.run_until_parked();
            if badges_shown(window, cx) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "badges never appeared within 5s of the held tab chord"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[gpui::test]
    fn deactivation_dismisses_visible_badges(cx: &mut TestAppContext) {
        let (window, mut vcx) = harness(cx);
        let hold = hold_modifiers(&window, cx);
        vcx.simulate_modifiers_change(hold);
        wait_for_badges(&window, cx);

        vcx.deactivate_window();

        assert!(
            !badges_shown(&window, cx),
            "deactivation must dismiss the badges — the modifier release will never reach this window"
        );
    }

    #[gpui::test]
    fn deactivation_cancels_a_pending_reveal(cx: &mut TestAppContext) {
        let (window, mut vcx) = harness(cx);
        let hold = hold_modifiers(&window, cx);
        vcx.simulate_modifiers_change(hold);
        vcx.deactivate_window();

        std::thread::sleep(std::time::Duration::from_millis(400));
        cx.background_executor.run_until_parked();

        assert!(
            !badges_shown(&window, cx),
            "a reveal scheduled before deactivation must not fire after it"
        );
    }

    /// The regression: the badges answered to the platform's secondary
    /// modifier rather than to the chord the numbers are drawn for. On Windows
    /// and Linux, where the nine ship on Alt, Ctrl was the only key that lit
    /// them — and a tab row rebound onto Alt, the case reported, raised nothing
    /// however long it was held.
    #[gpui::test]
    fn the_bound_modifier_raises_the_badges_and_another_one_does_not(cx: &mut TestAppContext) {
        let (window, mut vcx) = harness(cx);
        vcx.update(|_, cx| {
            // Exactly Alt+1…9, as the Settings page writes a rebind: a list,
            // so the shipped chord is gone rather than sitting beside it.
            cx.global_mut::<Config>().keybindings = serde_json::from_value(serde_json::json!({
                "ActivateTab1": ["alt-1"],
            }))
            .expect("the binding loads");
        });

        vcx.simulate_modifiers_change(Modifiers::control());
        std::thread::sleep(std::time::Duration::from_millis(400));
        cx.background_executor.run_until_parked();
        assert!(
            !badges_shown(&window, cx),
            "Ctrl is not the chord the tabs are numbered for here any more"
        );

        vcx.simulate_modifiers_change(Modifiers::none());
        vcx.simulate_modifiers_change(Modifiers::alt());
        wait_for_badges(&window, cx);
    }

    #[gpui::test]
    fn group_hold_shows_group_badges_and_all_hold_restores_global_ones(cx: &mut TestAppContext) {
        let (window, mut vcx) = harness(cx);
        let group_chord = if cfg!(target_os = "macos") {
            "secondary-shift-1"
        } else {
            "alt-shift-1"
        };
        let group = gpui::Keystroke::parse(group_chord)
            .expect("the shipped group chord parses")
            .modifiers;

        vcx.simulate_modifiers_change(group);
        std::thread::sleep(std::time::Duration::from_millis(400));
        cx.background_executor.run_until_parked();
        assert!(!badges_shown(&window, cx), "the feature ships off");

        vcx.update(|_, cx| {
            cx.global_mut::<Config>().tab_group_shortcuts = true;
            crate::ui::keymap::rebind(cx);
        });
        assert_eq!(
            window
                .update(cx, |_, _, cx| crate::ui::keymap::group_tab_badge_hold(cx))
                .unwrap(),
            Some(group)
        );
        vcx.simulate_modifiers_change(Modifiers::none());
        vcx.simulate_modifiers_change(group);
        wait_for_badges(&window, cx);
        assert!(
            window.update(cx, |app, _, _| app.mod_hint_group).unwrap(),
            "the group hold selects group-relative positions"
        );

        vcx.simulate_modifiers_change(Modifiers::none());
        assert!(
            !window.update(cx, |app, _, _| app.mod_hint_group).unwrap(),
            "releasing the modifier resets the group-hold flag alongside the badges"
        );

        let all = hold_modifiers(&window, cx);
        vcx.simulate_modifiers_change(all);
        wait_for_badges(&window, cx);
        assert!(
            !window.update(cx, |app, _, _| app.mod_hint_group).unwrap(),
            "the ordinary hold still numbers every tab"
        );
    }
}
