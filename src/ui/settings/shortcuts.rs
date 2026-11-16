//! Keyboard shortcuts: every action in the palette's groups, each with its
//! keys as keycaps. Click the keys to record new ones; a chord another action
//! already has asks before it is taken over.

use super::kit::{BtnKind, Tk, fs};
use super::*;

impl Tty59App {
    pub(crate) fn render_settings_keybindings(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = Tk::of(cx);
        let Some(s) = self.active_settings() else {
            return div().into_any_element();
        };
        let search = s.shortcut_search.clone();
        let query = search.read(cx).value().trim().to_lowercase();
        let recording = s
            .recording
            .as_ref()
            .map(|r| (r.action.clone(), r.chords.clone()));
        let conflict = s
            .kb_conflict
            .as_ref()
            .map(|c| (c.action.clone(), c.spec.clone(), c.other.clone()));
        let note = s.rebinding_note.clone();

        let (preset, prefix, overridden) = {
            let cfg = cx.global::<Config>();
            let overridden: std::collections::HashSet<String> =
                cfg.keybindings.keys().cloned().collect();
            (
                cfg.keybinding_preset.clone(),
                cfg.prefix.clone(),
                overridden,
            )
        };
        let tmux = preset == "tmux";
        let changed = overridden.len();
        let effective = crate::ui::keymap::effective_chords(cx);

        // The toolbar: the filter, a way back from every change at once, and
        // the preset the whole table starts from.
        let presets = self.settings_choice(
            "kb-preset",
            &[t(L10nKey::SettingsDefault), "tmux"],
            usize::from(tmux),
            cx,
            |this, ix, _w, cx| {
                this.set_keybinding_preset(if ix == 0 { "default" } else { "tmux" }, cx)
            },
        );
        let toolbar =
            h_flex()
                .gap(px(12.))
                .items_center()
                .child(
                    kit::search_field(&search, &tk)
                        .h(px(28.))
                        .flex_1()
                        .max_w(px(300.)),
                )
                .child(div().flex_1())
                .when(changed > 0, |r| {
                    r.child(
                        kit::button(
                            "kb-restore-all",
                            t_plural(L10nKey::SettingsRestoreChanged, changed, &[]),
                            BtnKind::Link,
                        )
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.restore_default_keybindings_confirmed(cx)
                        })),
                    )
                })
                .child(presets);
        // Backspace clears the shortcut being recorded, so it is said while
        // one is, not in the resting line.
        //
        // The two lines wrap differently in some languages and widths, so swapping one for the other would move
        // the whole list under the pointer. Both therefore sit in ONE grid cell: the cell is as tall as the taller
        // of the two at the current width, font and language (the layout engine sizes it, nothing is measured or
        // cached here), and the one not in use is invisible: laid out, never painted, no role, no accessibility node.
        let (resting, _resting_kind) = if tmux {
            (t(L10nKey::SettingsShortcutsHintTmux), "tmux")
        } else {
            (t(L10nKey::SettingsShortcutsHint), "idle")
        };
        let recording_now = recording.is_some();
        let copy = |text: &'static str, active: bool| {
            div()
                .col_start(1)
                .row_start(1)
                // Keep its own height (the cell is the taller one's), so a line's bounds are its own text's.
                .self_start()
                .text_size(fs(12.))
                .line_height(px(17.))
                .text_color(tk.sub)
                .when(!active, |d| d.invisible())
                .child(text)
        };
        let resting_copy = copy(resting, !recording_now);
        let recording_copy = copy(t(L10nKey::SettingsShortcutsRecordingHint), recording_now);
        // The probe and the selectors belong to the copy in use, so a test (and the paint probe) sees one line.
        #[cfg(test)]
        let (resting_copy, recording_copy) = if recording_now {
            let recording_copy = kit::probe("settings.shortcut-hint", recording_copy);
            let recording_copy =
                recording_copy.debug_selector(|| "settings-shortcut-hint-recording".to_string());
            (resting_copy, recording_copy)
        } else {
            let resting_copy = kit::probe("settings.shortcut-hint", resting_copy);
            let resting_copy = resting_copy
                .debug_selector(move || format!("settings-shortcut-hint-{_resting_kind}"));
            (resting_copy, recording_copy)
        };
        let hint = div()
            .grid()
            .grid_cols(1)
            .max_w(px(560.))
            .child(resting_copy)
            .child(recording_copy);
        #[cfg(test)]
        let hint = hint.debug_selector(|| "settings-shortcut-hint-box".to_string());
        let prefix_row = tmux.then(|| {
            h_flex()
                .gap(px(10.))
                .items_center()
                .child(
                    div()
                        .text_size(fs(12.))
                        .text_color(tk.sub)
                        .child(t(L10nKey::SettingsPrefix)),
                )
                .child(self.settings_choice(
                    "kb-prefix",
                    &["⌃B", "⌃A"],
                    usize::from(prefix == "ctrl-a"),
                    cx,
                    |this, ix, _w, cx| {
                        this.set_keybinding_prefix(if ix == 0 { "ctrl-b" } else { "ctrl-a" }, cx)
                    },
                ))
        });
        let intro = v_flex()
            .mt(px(-12.))
            .gap(px(10.))
            .child(toolbar)
            .child(hint)
            .children(prefix_row)
            .when_some(note, |v, note| {
                v.child(div().text_size(fs(12.)).text_color(tk.fg).child(note))
            })
            .into_any_element();

        // The rows, in the palette's groups.
        let mut grouped: Vec<(
            crate::ui::search::CommandGroup,
            Vec<(String, Vec<String>, String)>,
        )> = Vec::new();
        for (action, keys) in effective {
            let (group, label) = crate::ui::keymap::action_entry(&action);
            if !query.is_empty()
                && !keybinding_matches_query(&action, &query)
                && !label.to_lowercase().contains(&query)
            {
                continue;
            }
            match grouped.iter_mut().find(|(g, _)| *g == group) {
                Some((_, rows)) => rows.push((action, keys, label)),
                None => grouped.push((group, vec![(action, keys, label)])),
            }
        }
        grouped.sort_by_key(|(g, _)| {
            crate::ui::search::CommandGroup::ORDER
                .iter()
                .position(|o| o == g)
                .unwrap_or(usize::MAX)
        });

        let mut groups = vec![intro];
        let none = grouped.is_empty();
        for (group, rows) in grouped {
            let mut section = v_flex().child(
                div()
                    .pb(px(6.))
                    .text_size(fs(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tk.heading)
                    .child(group.title()),
            );
            for (action, keys, label) in rows {
                let is_recording = recording.as_ref().is_some_and(|(a, _)| *a == action);
                let conflict_here = conflict.as_ref().filter(|(a, _, _)| *a == action);
                let shown: Vec<String> = match (&recording, conflict_here) {
                    (Some((a, chords)), _) if *a == action => vec![chords.join(" ")]
                        .into_iter()
                        .filter(|c| !c.is_empty())
                        .collect(),
                    (_, Some((_, spec, _))) => vec![spec.clone()],
                    _ => keys,
                };
                // "New Tab, ⌘T": the action and what it is bound to, the way
                // the row reads — a screen reader says ⌘ as "command".
                let unassigned = !is_recording && shown.is_empty();
                let a11y_keys = match shown.is_empty() {
                    true if is_recording => t(L10nKey::SettingsPressKeysShort).to_string(),
                    true => t(L10nKey::SettingsShortcutAdd).to_string(),
                    false => shown
                        .iter()
                        .map(|spec| crate::ui::keymap::key_tokens(spec).join(""))
                        .collect::<Vec<_>>()
                        .join(" "),
                };
                let a11y_name = format!("{label}, {a11y_keys}");
                let modified = overridden.contains(&action) && !is_recording;
                let ring = if is_recording {
                    vec![kit::ring(tk.fg, 1., true)]
                } else if conflict_here.is_some() {
                    vec![kit::ring(tk.warn, 1., true)]
                } else {
                    Vec::new()
                };
                let hover_ring = if is_recording || conflict_here.is_some() {
                    vec![kit::ring(tk.fg, 1., true)]
                } else {
                    vec![kit::ring(tk.k15, 0.5, true)]
                };
                let focus_ring = hover_ring.clone();
                let record_action = action.clone();
                let keys_button = h_flex()
                    .id(SharedString::from(format!("kb-{action}")))
                    // Tab reaches the cell and Enter or Space starts recording
                    // through the click handler below, as a click does.
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .aria_label(a11y_name)
                    .h(px(26.))
                    .min_w(px(72.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .gap(px(5.))
                    .items_center()
                    .justify_end()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(is_recording, |b| b.bg(tk.page))
                    .shadow(ring)
                    // An unassigned cell is empty until the pointer or the
                    // keyboard is on it; "Add" inherits this ink, so showing
                    // it moves nothing.
                    .when(unassigned, |b| b.text_color(gpui::transparent_black()))
                    .hover(move |b| {
                        let b = b.shadow(hover_ring.clone());
                        if unassigned { b.text_color(tk.sub) } else { b }
                    })
                    // A2 handoff: replace this focus indicator with the app-wide ring.
                    .focus_visible(move |b| {
                        let b = b.shadow(focus_ring.clone());
                        if unassigned { b.text_color(tk.sub) } else { b }
                    })
                    .when(is_recording && shown.is_empty(), |b| {
                        b.child(
                            div()
                                .pr(px(2.))
                                .text_size(fs(12.))
                                .text_color(tk.sub)
                                .child(t(L10nKey::SettingsPressKeysShort)),
                        )
                    })
                    .children(shown.iter().enumerate().map(|(n, spec)| {
                        h_flex()
                            .gap(px(5.))
                            .items_center()
                            .when(n > 0, |d| {
                                d.child(div().text_size(fs(11.)).text_color(tk.sub).child("/"))
                            })
                            .child(Self::keycap_row(spec, &tk))
                    }))
                    .when(unassigned, |b| {
                        let add = div()
                            .pr(px(4.))
                            .text_size(fs(12.))
                            .child(t(L10nKey::SettingsShortcutAdd));
                        #[cfg(test)]
                        let add = {
                            let selector = format!("settings-shortcut-add-{action}");
                            add.debug_selector(move || selector)
                        };
                        b.child(add)
                    })
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                        let already = this
                            .active_settings()
                            .and_then(|s| s.recording.as_ref())
                            .is_some_and(|r| r.action == record_action);
                        if already {
                            // Enter and Space click on key-up, and while this
                            // cell records they are chords: the key-up of one
                            // just taken must not read as a click that stops
                            // the recording it belongs to.
                            if ev.is_keyboard() {
                                return;
                            }
                            if let Some(s) = this.active_settings_mut() {
                                s.recording = None;
                            }
                            cx.notify();
                        } else {
                            this.start_recording_key(record_action.clone(), window, cx)
                        }
                    }));
                #[cfg(test)]
                let keys_button = kit::probe("settings.shortcut-cell", keys_button);
                #[cfg(test)]
                let keys_button = {
                    let selector = format!("kb-{action}");
                    keys_button.debug_selector(move || selector)
                };
                let reset_action = action.clone();
                let row = h_flex()
                    .h(px(34.))
                    .px(px(8.))
                    .gap(px(16.))
                    .items_center()
                    .justify_between()
                    .rounded(px(7.))
                    .when(conflict_here.is_none(), |r| r.hover(move |s| s.bg(tk.k04)))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(8.))
                            .items_center()
                            .child(div().min_w_0().truncate().child(label))
                            .when(modified, |r| {
                                r.child(
                                    div()
                                        .id(SharedString::from(format!("reset-{action}")))
                                        .flex_shrink_0()
                                        .text_size(fs(11.5))
                                        .text_color(tk.sub)
                                        .cursor_pointer()
                                        .hover(move |s| s.text_color(tk.fg))
                                        .child(t(L10nKey::Reset))
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.reset_keybinding(reset_action.clone(), cx)
                                        })),
                                )
                            }),
                    )
                    .child(keys_button);
                let conflict_line = conflict_here.map(|(_, spec, other)| {
                    let keys = crate::ui::keymap::key_chords(spec)
                        .into_iter()
                        .map(|chord| chord.concat())
                        .collect::<Vec<_>>()
                        .join(&format!(" {} ", t(L10nKey::SettingsKeyThen)));
                    let other = crate::ui::keymap::action_entry(other)
                        .1
                        .trim_end_matches('…')
                        .to_string();
                    h_flex()
                        .px(px(8.))
                        .pt(px(2.))
                        .pb(px(10.))
                        .gap(px(8.))
                        .items_center()
                        .text_size(fs(12.))
                        .text_color(tk.sub)
                        .child(
                            div()
                                .size(px(6.))
                                .flex_shrink_0()
                                .rounded_full()
                                .bg(tk.warn),
                        )
                        .child(div().flex_1().min_w_0().child(t_fmt(
                            L10nKey::SettingsShortcutConflict,
                            &[("keys", &keys), ("action", &other)],
                        )))
                        .child(
                            kit::button(
                                "kb-conflict-replace",
                                t(L10nKey::SettingsReplace),
                                BtnKind::Primary,
                            )
                            .text_size(fs(12.))
                            .on_click(cx.listener(
                                |this, _, _w, cx| this.resolve_keybinding_conflict(true, cx),
                            )),
                        )
                        .child(
                            kit::button(
                                "kb-conflict-cancel",
                                t(L10nKey::Cancel),
                                BtnKind::Secondary,
                            )
                            .text_size(fs(12.))
                            .on_click(cx.listener(
                                |this, _, _w, cx| this.resolve_keybinding_conflict(false, cx),
                            )),
                        )
                });
                section = section.child(
                    v_flex()
                        .mx(px(-8.))
                        .rounded(px(7.))
                        .when(conflict_line.is_some(), |d| d.bg(tk.k04))
                        .child(row)
                        .children(conflict_line),
                );
            }
            groups.push(section.into_any_element());
        }
        if none {
            groups.push(
                div()
                    .text_color(tk.sub)
                    .child(t_fmt(L10nKey::SettingsNoActionsMatch, &[("query", &query)]))
                    .into_any_element(),
            );
        }
        Self::settings_page(groups)
    }
}

use super::kit;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::paint_probe;
    use gpui::{Entity, KeyUpEvent, Keystroke, Modifiers, TestAppContext, VisualTestContext, size};
    use kit::test_support::harness;

    /// Two paints: the first lays the page out, the second reads the bounds
    /// and focus that layout produced.
    fn settle(vcx: &mut VisualTestContext) {
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
    }

    /// A key press as the keyboard delivers it: down, then up.
    fn press(vcx: &mut VisualTestContext, key: &str) {
        vcx.simulate_keystrokes(key);
        vcx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("a key name"),
        });
        vcx.run_until_parked();
    }

    fn recording(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
    ) -> Option<(String, Vec<String>)> {
        app.update_in(vcx, |app, _, _| {
            app.active_settings()
                .and_then(|s| s.recording.as_ref())
                .map(|r| (r.action.clone(), r.chords.clone()))
        })
    }

    /// The Keyboard Shortcuts page in a window tall enough that every row is
    /// on screen, so bounds and clicks need no scrolling, with New Tab
    /// unassigned the way a person unassigns it: record it, press Backspace.
    fn page_with_new_tab_unassigned(
        cx: &mut TestAppContext,
    ) -> (Entity<Tty59App>, VisualTestContext) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::Keybindings, window, cx);
        });
        vcx.simulate_resize(size(px(1100.), px(4000.)));
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.start_recording_key("NewTab".to_string(), window, cx);
        });
        vcx.simulate_keystrokes("backspace");
        settle(&mut vcx);
        assert!(
            vcx.update(|_, cx| crate::ui::keymap::effective_key("NewTab", cx))
                .is_none(),
            "Backspace on a row that recorded nothing leaves the action unassigned"
        );
        (app, vcx)
    }

    fn new_tab_cell(vcx: &mut VisualTestContext) -> gpui::Bounds<gpui::Pixels> {
        vcx.debug_bounds("kb-NewTab")
            .expect("the New Tab shortcut cell is drawn")
    }

    /// A real mouse click on New Tab's cell *where it is now*. Starting or
    /// stopping a recording swaps the helper line and the cell's own content;
    /// the helper box is as tall as the taller of its two lines, so the list
    /// no longer moves (`the_list_does_not_move_when_recording_starts_or_stops`
    /// pins that), but the cell is still looked up again after the last layout
    /// settled, never from an earlier look, and the point clicked is asserted
    /// to be inside the cell that was drawn.
    fn click_new_tab(vcx: &mut VisualTestContext) {
        settle(vcx);
        let cell = new_tab_cell(vcx);
        let point = cell.center();
        assert!(cell.contains(&point), "{point:?} is in {cell:?}");
        vcx.simulate_click(point, Modifiers::default());
        settle(vcx);
    }

    /// F20: exactly the unassigned cells carry no ink at rest (their "Add"
    /// inherits it and is shown by hover or focus), and no assigned cell is
    /// touched.
    #[gpui::test]
    fn only_unassigned_cells_rest_without_ink(cx: &mut TestAppContext) {
        let _probe = paint_probe::scope();
        let (_app, mut vcx) = page_with_new_tab_unassigned(cx);
        settle(&mut vcx);
        // Exactly one render pass is observed: the notes are dropped and the
        // page is drawn once, inside one update, so no earlier or implicit
        // pass (a notify flushes its own draw) can add a second set. Two sets
        // of 193 notes read as 386 once, and that is not a count to accept.
        vcx.update(|window, cx| {
            paint_probe::clear();
            window.refresh();
            let _ = window.draw(cx);
        });
        let (total, unassigned) = vcx.update(|_, cx| {
            let rows = crate::ui::keymap::effective_chords(cx);
            (
                rows.len(),
                rows.iter().filter(|(_, keys)| keys.is_empty()).count(),
            )
        });
        assert!(unassigned >= 1, "New Tab is unassigned");
        let cells = paint_probe::all("settings.shortcut-cell");
        assert_eq!(cells.len(), total, "one cell per action");
        let blank = cells.iter().filter(|c| c.text_alpha == Some(0.)).count();
        assert_eq!(blank, unassigned, "one blank cell per unassigned action");
        assert!(
            cells
                .iter()
                .filter(|c| c.text_alpha != Some(0.))
                .all(|c| c.text.is_none()),
            "an assigned cell keeps the ink it inherits"
        );
    }

    /// F20: the empty cell is the same 26px control it was, "Add" sits inside
    /// it, and pointing at it moves nothing.
    #[gpui::test]
    fn add_sits_in_the_empty_cell_and_pointing_at_it_moves_nothing(cx: &mut TestAppContext) {
        let (_app, mut vcx) = page_with_new_tab_unassigned(cx);
        let cell = new_tab_cell(&mut vcx);
        let add = vcx
            .debug_bounds("settings-shortcut-add-NewTab")
            .expect("Add is laid out in the unassigned cell");
        assert!(
            cell.contains(&add.center()),
            "Add {add:?} is inside the cell {cell:?}"
        );
        assert_eq!(cell.size.height, px(26.));
        assert!(cell.size.width >= px(72.), "{cell:?}");

        vcx.simulate_mouse_move(cell.center(), None, Modifiers::default());
        settle(&mut vcx);
        assert_eq!(vcx.debug_bounds("kb-NewTab"), Some(cell), "the cell");
        assert_eq!(
            vcx.debug_bounds("settings-shortcut-add-NewTab"),
            Some(add),
            "and Add stay put under the pointer"
        );
    }

    /// F20: the keyboard reaches the cell and records through the same entry a
    /// click does. A click focuses the cell; after Esc, Enter (key down and up)
    /// starts recording again; Tab and Shift-Tab move between cells and Enter
    /// records for whichever one has focus.
    #[gpui::test]
    fn tab_and_enter_record_through_the_same_entry_as_a_click(cx: &mut TestAppContext) {
        let (app, mut vcx) = page_with_new_tab_unassigned(cx);

        click_new_tab(&mut vcx);
        assert_eq!(
            recording(&app, &mut vcx),
            Some(("NewTab".to_string(), Vec::new())),
            "a click starts recording"
        );
        press(&mut vcx, "escape");
        settle(&mut vcx);
        assert_eq!(recording(&app, &mut vcx), None, "Esc cancels");
        assert!(
            vcx.update(|_, cx| crate::ui::keymap::effective_key("NewTab", cx))
                .is_none(),
            "and records nothing"
        );

        press(&mut vcx, "enter");
        settle(&mut vcx);
        assert_eq!(
            recording(&app, &mut vcx),
            Some(("NewTab".to_string(), Vec::new())),
            "Enter, down and up, on the focused cell starts recording"
        );
        press(&mut vcx, "escape");
        settle(&mut vcx);

        vcx.update(|window, cx| window.focus_next(cx));
        settle(&mut vcx);
        press(&mut vcx, "enter");
        settle(&mut vcx);
        let (next, _) =
            recording(&app, &mut vcx).expect("Tab reaches the next cell and Enter records for it");
        assert_ne!(next, "NewTab", "the next stop is another action's cell");
        press(&mut vcx, "escape");
        settle(&mut vcx);

        vcx.update(|window, cx| window.focus_prev(cx));
        settle(&mut vcx);
        press(&mut vcx, "enter");
        settle(&mut vcx);
        assert_eq!(
            recording(&app, &mut vcx).map(|(action, _)| action),
            Some("NewTab".to_string()),
            "and Shift-Tab comes back"
        );
        press(&mut vcx, "escape");
        settle(&mut vcx);
        assert_eq!(recording(&app, &mut vcx), None);
    }

    /// F20: while a cell records, Enter is a chord, and the key-up that
    /// follows it must not click the focused cell and stop the recording.
    #[gpui::test]
    fn the_key_up_of_a_recorded_enter_does_not_stop_the_recording(cx: &mut TestAppContext) {
        let (app, mut vcx) = page_with_new_tab_unassigned(cx);
        click_new_tab(&mut vcx);
        assert!(recording(&app, &mut vcx).is_some(), "recording");

        vcx.simulate_keystrokes("enter");
        settle(&mut vcx);
        assert_eq!(
            recording(&app, &mut vcx),
            Some(("NewTab".to_string(), vec!["enter".to_string()])),
            "Enter went in as a chord"
        );
        vcx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("enter").expect("a key name"),
        });
        settle(&mut vcx);
        assert_eq!(
            recording(&app, &mut vcx),
            Some(("NewTab".to_string(), vec!["enter".to_string()])),
            "its key-up did not stop the recording or lose the chord"
        );

        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert_eq!(recording(&app, &mut vcx), None, "Esc still cancels");
    }

    /// F20, unchanged behaviour next to the new cell: a second mouse click on
    /// the recording cell stops it, and Backspace with nothing recorded clears
    /// the action and ends the recording.
    #[gpui::test]
    fn a_second_click_stops_and_backspace_clears(cx: &mut TestAppContext) {
        let (app, mut vcx) = page_with_new_tab_unassigned(cx);

        click_new_tab(&mut vcx);
        assert!(recording(&app, &mut vcx).is_some());
        // The cell and the rows under the helper line have been re-laid out
        // for the recording state; the second click goes to where it is now.
        click_new_tab(&mut vcx);
        assert_eq!(
            recording(&app, &mut vcx),
            None,
            "a mouse click on the recording cell toggles it off"
        );

        click_new_tab(&mut vcx);
        assert!(recording(&app, &mut vcx).is_some());
        vcx.simulate_keystrokes("backspace");
        settle(&mut vcx);
        assert_eq!(recording(&app, &mut vcx), None, "Backspace ends it");
        assert!(
            vcx.update(|_, cx| crate::ui::keymap::effective_key("NewTab", cx))
                .is_none(),
            "and the action stays unassigned"
        );
    }

    /// F20: the resting helper line has no Backspace in it, the recording one
    /// does, in every locale; and the line stays inside 560px and the window
    /// in each, resting or recording.
    #[gpui::test]
    fn the_helper_changes_while_recording_and_fits_in_every_locale(cx: &mut TestAppContext) {
        let (app, mut vcx) = page_with_new_tab_unassigned(cx);
        assert!(vcx.debug_bounds("settings-shortcut-hint-idle").is_some());
        assert!(
            vcx.debug_bounds("settings-shortcut-hint-recording")
                .is_none()
        );
        click_new_tab(&mut vcx);
        assert!(
            vcx.debug_bounds("settings-shortcut-hint-recording")
                .is_some(),
            "while recording the helper says how to cancel and clear"
        );
        assert!(vcx.debug_bounds("settings-shortcut-hint-idle").is_none());
        press(&mut vcx, "escape");
        settle(&mut vcx);

        let viewport = vcx.update(|window, _| window.viewport_size());
        for locale in ["en", "ru-RU", "zh-CN", "ja-JP"] {
            crate::ui::i18n::set_locale(locale);
            for state in ["idle", "recording"] {
                if state == "recording" {
                    app.update_in(&mut vcx, |app, window, cx| {
                        app.start_recording_key("NewTab".to_string(), window, cx);
                    });
                } else {
                    app.update_in(&mut vcx, |_, _, cx| cx.notify());
                }
                settle(&mut vcx);
                let selector = match state {
                    "idle" => "settings-shortcut-hint-idle",
                    _ => "settings-shortcut-hint-recording",
                };
                let hint = vcx
                    .debug_bounds(selector)
                    .unwrap_or_else(|| panic!("{locale} {state}: the helper is drawn"));
                assert!(
                    hint.size.width <= px(560.5),
                    "{locale} {state}: {hint:?} is capped at 560px"
                );
                assert!(
                    hint.origin.x >= px(0.) && hint.right() <= viewport.width,
                    "{locale} {state}: {hint:?} stays in the window"
                );
                assert!(
                    hint.size.height >= px(17.) && hint.size.height <= px(51.),
                    "{locale} {state}: {hint:?} is one to three 17px lines, not clipped or shredded"
                );
                if state == "recording" {
                    press(&mut vcx, "escape");
                    settle(&mut vcx);
                }
            }
        }
        crate::ui::i18n::set_locale("en");
    }

    /// The y and height of the two rows the recording tests look at: New Tab
    /// (never the one recording) and Close Pane / Tab (the one recording).
    fn list_rows(vcx: &mut VisualTestContext) -> Option<[gpui::Bounds<gpui::Pixels>; 2]> {
        Some([
            vcx.debug_bounds("kb-NewTab")?,
            vcx.debug_bounds("kb-CloseActiveTab")?,
        ])
    }

    /// Same y and height within half a pixel, row by row. The x and the width
    /// are NOT compared: "Press keys…" legitimately makes the recording cell
    /// wider than "Add".
    fn same_rows(a: &[gpui::Bounds<gpui::Pixels>; 2], b: &[gpui::Bounds<gpui::Pixels>; 2]) -> bool {
        a.iter().zip(b).all(|(x, y)| {
            (x.origin.y - y.origin.y).abs() <= px(0.5)
                && (x.size.height - y.size.height).abs() <= px(0.5)
        })
    }

    /// F20: starting or stopping a recording swaps the helper line above the
    /// list, and in some languages and widths the two lines wrap to different
    /// numbers of lines. The list below must not move when that happens, or
    /// the pointer that just clicked a cell ends up on the next row. 16 states:
    /// 4 languages x 2 window sizes x 2 interface fonts, a real render each,
    /// recording started through the production entry and stopped with a real
    /// Escape. Every state that moves is reported, not just the first.
    #[gpui::test]
    fn the_list_does_not_move_when_recording_starts_or_stops(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::Keybindings, window, cx);
        });
        let mut states = 0;
        let mut moved: Vec<String> = Vec::new();
        for locale in ["en", "ru-RU", "zh-CN", "ja-JP"] {
            crate::ui::i18n::set_locale(locale);
            for (w, h) in [(720., 480.), (960., 600.)] {
                for font in [13., 22.] {
                    states += 1;
                    let state = format!("{locale} {w}x{h} @{font}pt");
                    vcx.simulate_resize(size(px(w), px(h)));
                    app.update_in(&mut vcx, |_, _, cx| {
                        cx.global_mut::<crate::core::config::Config>().ui_font_size = font;
                        cx.notify();
                    });
                    settle(&mut vcx);
                    let Some(idle) = list_rows(&mut vcx) else {
                        moved.push(format!("{state}: the rows are not drawn"));
                        continue;
                    };
                    app.update_in(&mut vcx, |app, window, cx| {
                        app.start_recording_key("CloseActiveTab".to_string(), window, cx);
                    });
                    settle(&mut vcx);
                    let during = list_rows(&mut vcx);
                    if recording(&app, &mut vcx)
                        .map(|(action, _)| action)
                        .as_deref()
                        != Some("CloseActiveTab")
                    {
                        moved.push(format!("{state}: the recording did not start"));
                    }
                    vcx.simulate_keystrokes("escape");
                    settle(&mut vcx);
                    let after = list_rows(&mut vcx);
                    if recording(&app, &mut vcx).is_some() {
                        moved.push(format!("{state}: Esc did not stop the recording"));
                    }
                    match (during, after) {
                        (Some(during), Some(after)) => {
                            if !same_rows(&idle, &during) {
                                moved.push(format!(
                                    "{state}: starting a recording moved the list: {idle:?} -> {during:?}"
                                ));
                            }
                            if !same_rows(&idle, &after) {
                                moved.push(format!(
                                    "{state}: stopping it moved the list: {idle:?} -> {after:?}"
                                ));
                            }
                        }
                        _ => moved.push(format!("{state}: the rows vanished while recording")),
                    }
                }
            }
        }
        crate::ui::i18n::set_locale("en");
        assert_eq!(states, 16, "4 languages x 2 sizes x 2 fonts");
        assert!(
            moved.is_empty(),
            "the list moved in {} of {states} states:\n{}",
            moved.len(),
            moved.join("\n")
        );
    }

    /// F20: the helper box is exactly as tall as the taller of its two lines
    /// at the current width, font and language, and the same height whether
    /// the resting or the recording line is the one in use: the reservation
    /// itself, measured on the box (the one thing only the fix draws). 16
    /// states, as above; the in-use line's own height is read from its
    /// selector, the unused one is invisible and has none.
    #[gpui::test]
    fn the_hint_box_is_as_tall_as_the_taller_line_in_both_states(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::Keybindings, window, cx);
        });
        let mut states = 0;
        let mut bad: Vec<String> = Vec::new();
        for locale in ["en", "ru-RU", "zh-CN", "ja-JP"] {
            crate::ui::i18n::set_locale(locale);
            for (w, h) in [(720., 480.), (960., 600.)] {
                for font in [13., 22.] {
                    states += 1;
                    let state = format!("{locale} {w}x{h} @{font}pt");
                    vcx.simulate_resize(size(px(w), px(h)));
                    app.update_in(&mut vcx, |_, _, cx| {
                        cx.global_mut::<crate::core::config::Config>().ui_font_size = font;
                        cx.notify();
                    });
                    settle(&mut vcx);
                    let resting = (
                        vcx.debug_bounds("settings-shortcut-hint-box"),
                        vcx.debug_bounds("settings-shortcut-hint-idle"),
                    );
                    app.update_in(&mut vcx, |app, window, cx| {
                        app.start_recording_key("CloseActiveTab".to_string(), window, cx);
                    });
                    settle(&mut vcx);
                    let recording = (
                        vcx.debug_bounds("settings-shortcut-hint-box"),
                        vcx.debug_bounds("settings-shortcut-hint-recording"),
                    );
                    vcx.simulate_keystrokes("escape");
                    settle(&mut vcx);
                    let (Some(rest_box), Some(rest_line), Some(rec_box), Some(rec_line)) =
                        (resting.0, resting.1, recording.0, recording.1)
                    else {
                        bad.push(format!("{state}: the box or the line in use is not drawn"));
                        continue;
                    };
                    let taller = rest_line.size.height.max(rec_line.size.height);
                    if (rest_box.size.height - rec_box.size.height).abs() > px(0.5) {
                        bad.push(format!(
                            "{state}: the box is {:?} high resting and {:?} recording",
                            rest_box.size.height, rec_box.size.height
                        ));
                    }
                    if (rest_box.size.height - taller).abs() > px(0.5) {
                        bad.push(format!(
                            "{state}: the box is {:?} high, the taller line is {:?}",
                            rest_box.size.height, taller
                        ));
                    }
                    if rest_box.size.width > px(560.5) {
                        bad.push(format!(
                            "{state}: the box is {:?} wide, over 560px",
                            rest_box.size.width
                        ));
                    }
                }
            }
        }
        crate::ui::i18n::set_locale("en");
        assert_eq!(states, 16, "4 languages x 2 sizes x 2 fonts");
        assert!(
            bad.is_empty(),
            "{} of {states} states:\n{}",
            bad.len(),
            bad.join("\n")
        );
    }

    /// F20: the helper catalog says Backspace only for the recording state,
    /// in every shipped locale.
    #[test]
    fn backspace_is_taught_while_recording_not_at_rest() {
        for locale in ["en", "ru-RU", "zh-CN", "ja-JP"] {
            crate::ui::i18n::set_locale(locale);
            for key in [
                L10nKey::SettingsShortcutsHint,
                L10nKey::SettingsShortcutsHintTmux,
            ] {
                assert!(
                    !t(key).contains("Backspace") && !t(key).contains('⌫'),
                    "{locale}: the resting helper {:?} leaves Backspace to recording",
                    t(key)
                );
            }
            assert!(
                t(L10nKey::SettingsShortcutsRecordingHint).contains("Backspace"),
                "{locale}: the recording helper names Backspace"
            );
        }
        crate::ui::i18n::set_locale("en");
    }
}
