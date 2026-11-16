//! A3.3 (settings F14 / F21), tests first. Drives the real settings render and
//! the real Reset click. Test-only seams the change adds under `cfg(test)`:
//! the row's `-reset` / `-modified-tag` selectors, the dropdown trigger's
//! `{id}-trigger` selector, and `paint_probe::label` for button labels (the
//! last already exists).
//!
//! Semantics the tests pin down:
//! * A theme slot row reflects the value it shows: System mode shows the
//!   slot's own key; a pinned Light/Dark mode shows `theme_preset`.
//! * Reset on a shown slot restores the default of that slot (also writing the
//!   active manual preset when pinned). Reset on an inactive slot (hidden in a
//!   pinned mode) writes that slot only: the visible theme, the follow-system
//!   flag and the manual preset stay put.
//! * In System mode a stale manual `theme_preset` is not a modification.
//! * A hidden slot drawn for a search or a jump has a disabled control; its
//!   Reset still works.
//! * Machine is session state: searchable, never modified, never Reset.

use super::kit::test_support::harness;
use super::*;
use crate::ui::paint_probe;
use gpui::{Modifiers, TestAppContext, VisualTestContext, size};

/// The rows under test. `debug_bounds` wants `&'static str`, so every selector
/// is a literal in one finite table rather than a formatted string.
#[derive(Clone, Copy, Debug)]
enum Slot {
    Light,
    Dark,
    Machine,
}

impl Slot {
    fn row(self) -> &'static str {
        match self {
            Slot::Light => "settings-row-SettingsLightThemeLabel",
            Slot::Dark => "settings-row-SettingsDarkThemeLabel",
            Slot::Machine => "settings-row-SettingsMachine",
        }
    }
    fn reset(self) -> &'static str {
        match self {
            Slot::Light => "settings-row-SettingsLightThemeLabel-reset",
            Slot::Dark => "settings-row-SettingsDarkThemeLabel-reset",
            Slot::Machine => "settings-row-SettingsMachine-reset",
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Slot::Light => "settings-row-SettingsLightThemeLabel-modified-tag",
            Slot::Dark => "settings-row-SettingsDarkThemeLabel-modified-tag",
            Slot::Machine => "settings-row-SettingsMachine-modified-tag",
        }
    }
    fn result(self) -> &'static str {
        match self {
            Slot::Light => "search-result-SettingsLightThemeLabel",
            Slot::Dark => "search-result-SettingsDarkThemeLabel",
            Slot::Machine => "search-result-SettingsMachine",
        }
    }
    /// The "Open setting" stub a result falls back to when its row was not
    /// built for the list.
    fn open_stub(self) -> &'static str {
        match self {
            Slot::Light => "search-open-SettingsLightThemeLabel",
            Slot::Dark => "search-open-SettingsDarkThemeLabel",
            Slot::Machine => "search-open-SettingsMachine",
        }
    }
}

const ALL_SLOTS: [Slot; 2] = [Slot::Light, Slot::Dark];

fn has(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    vcx.debug_bounds(selector).is_some()
}

fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    let at = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    vcx.simulate_click(at.center(), Modifiers::none());
    vcx.run_until_parked();
}

/// Config::default's theme ids and one non-default light and dark builtin.
struct Ids {
    default_light: String,
    default_dark: String,
    custom_light: String,
    custom_dark: String,
}

fn ids(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Ids {
    app.update(vcx, |_, cx| {
        let d = Config::default();
        let all = presets::all(cx);
        let pick = |dark: bool| {
            all.iter()
                .find(|t| {
                    t.dark == dark
                        && t.id != d.theme_preset_light
                        && t.id != d.theme_preset_dark
                        && t.id != d.theme_preset
                })
                .unwrap_or_else(|| panic!("no non-default builtin with dark={dark}"))
                .id
                .clone()
        };
        Ids {
            default_light: d.theme_preset_light.clone(),
            default_dark: d.theme_preset_dark.clone(),
            custom_light: pick(false),
            custom_dark: pick(true),
        }
    })
}

/// `(follow_system, manual, light, dark)` straight into the config, then the
/// Appearance page drawn tall enough that every row is laid out.
fn open_appearance(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    (follow, manual, light, dark): (bool, &str, &str, &str),
) {
    app.update_in(vcx, |app, window, cx| {
        let cfg = cx.global_mut::<Config>();
        cfg.theme_follow_system = follow;
        cfg.theme_preset = manual.into();
        cfg.theme_preset_light = light.into();
        cfg.theme_preset_dark = dark.into();
        app.open_settings_section(SettingsSection::Appearance, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    vcx.run_until_parked();
}

fn cfg_theme(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
) -> (bool, String, String, String) {
    app.update(vcx, |_, cx| {
        let c = cx.global::<Config>();
        (
            c.theme_follow_system,
            c.theme_preset.clone(),
            c.theme_preset_light.clone(),
            c.theme_preset_dark.clone(),
        )
    })
}

fn set_query(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, text: &str, modified_only: bool) {
    app.update_in(vcx, |app, window, cx| {
        app.active_settings_mut().unwrap().modified_only = modified_only;
        let search = app.active_settings().unwrap().search.clone();
        search.update(cx, |i, cx| i.set_value(text.to_string(), window, cx));
        app.autoselect_settings_search(cx);
    });
    vcx.run_until_parked();
}

/// System mode: each slot row has its own Reset and it resets that slot alone.
#[gpui::test]
fn system_mode_slot_reset_restores_only_that_slot(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    let id = ids(&app, &mut vcx);
    open_appearance(
        &app,
        &mut vcx,
        (true, &id.default_dark, &id.custom_light, &id.custom_dark),
    );
    assert!(has(&mut vcx, Slot::Light.row()) && has(&mut vcx, Slot::Dark.row()));

    click(&mut vcx, Slot::Dark.reset());
    let (follow, manual, light, dark) = cfg_theme(&app, &mut vcx);
    assert!(follow, "System mode survives a slot Reset");
    assert_eq!(
        dark, id.default_dark,
        "the dark slot is back to its default"
    );
    assert_eq!(light, id.custom_light, "the light slot is untouched");
    assert_eq!(manual, id.default_dark, "the manual preset is untouched");
    assert!(
        !has(&mut vcx, Slot::Dark.reset()),
        "a slot at its default no longer offers Reset"
    );
    assert!(has(&mut vcx, Slot::Light.reset()));

    click(&mut vcx, Slot::Light.reset());
    assert_eq!(cfg_theme(&app, &mut vcx).2, id.default_light);
}

/// Pinned Dark: the one visible row shows `theme_preset`. A custom manual theme
/// with both slots at their defaults is a modification of that row, and Reset
/// restores the default dark theme as both the manual preset and the slot.
#[gpui::test]
fn pinned_mode_row_reflects_and_resets_the_manual_theme(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    let id = ids(&app, &mut vcx);
    open_appearance(
        &app,
        &mut vcx,
        (false, &id.custom_dark, &id.default_light, &id.default_dark),
    );
    assert!(
        has(&mut vcx, Slot::Dark.row()),
        "pinned Dark shows the dark slot"
    );
    assert!(!has(&mut vcx, Slot::Light.row()), "and hides the light one");

    click(&mut vcx, Slot::Dark.reset());
    let (follow, manual, light, dark) = cfg_theme(&app, &mut vcx);
    assert!(!follow, "pinned mode survives the Reset");
    assert_eq!(manual, id.default_dark, "the visible theme is the default");
    assert_eq!(dark, id.default_dark);
    assert_eq!(light, id.default_light);
}

/// The reverse: System mode with a stale manual preset and slots at defaults
/// shows nothing modified anywhere, including the modified-only list.
#[gpui::test]
fn system_mode_ignores_a_stale_manual_preset(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    let id = ids(&app, &mut vcx);
    open_appearance(
        &app,
        &mut vcx,
        (true, &id.custom_dark, &id.default_light, &id.default_dark),
    );
    for slot in ALL_SLOTS {
        assert!(!has(&mut vcx, slot.reset()), "{slot:?}: Reset");
        assert!(!has(&mut vcx, slot.tag()), "{slot:?}: Modified tag");
    }
    set_query(&app, &mut vcx, "", true);
    for slot in ALL_SLOTS {
        assert!(
            !has(&mut vcx, slot.result()),
            "{slot:?} is not in the modified list"
        );
    }
    assert!(
        !has(&mut vcx, "search-result-SettingsThemeIntroTitle"),
        "the retired group-title entry is gone"
    );
}

/// Pinned Dark with a changed (hidden) light slot: the modified-only list
/// offers Reset, not "Open setting", and resetting it must not switch the
/// theme on screen.
#[gpui::test]
fn resetting_the_inactive_slot_keeps_the_visible_theme(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    let id = ids(&app, &mut vcx);
    open_appearance(
        &app,
        &mut vcx,
        (false, &id.custom_dark, &id.custom_light, &id.custom_dark),
    );
    set_query(&app, &mut vcx, "", true);
    assert!(has(&mut vcx, Slot::Light.result()));
    assert!(
        !has(&mut vcx, Slot::Light.open_stub()),
        "the hidden slot has its real row in the list, not an Open setting stub"
    );
    click(&mut vcx, Slot::Light.reset());
    let (follow, manual, light, dark) = cfg_theme(&app, &mut vcx);
    assert_eq!(light, id.default_light, "the inactive slot is reset");
    assert!(!follow, "still pinned");
    assert_eq!(manual, id.custom_dark, "the visible theme did not change");
    assert_eq!(dark, id.custom_dark, "the active slot did not change");
    let effective = app.update(&mut vcx, |_, cx| crate::ui::theme::effective_preset_id(cx));
    assert_eq!(effective, id.custom_dark);
}

/// Search for the slot that is hidden in the pinned mode still lands on its
/// actual row, and that row's control is really disabled, not just dimmed:
/// clicking it opens no menu and changes no theme.
#[gpui::test]
fn navigating_to_a_hidden_slot_draws_its_row_with_a_disabled_control(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    let id = ids(&app, &mut vcx);
    open_appearance(
        &app,
        &mut vcx,
        (false, &id.custom_dark, &id.default_light, &id.default_dark),
    );
    assert!(!has(&mut vcx, Slot::Light.row()));
    let before = cfg_theme(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.navigate_settings(
            SettingsSection::Appearance,
            Some(L10nKey::SettingsLightThemeLabel),
            window,
            cx,
        );
    });
    vcx.run_until_parked();
    assert!(
        has(&mut vcx, Slot::Light.row()),
        "the targeted slot is drawn even though the mode hides it"
    );
    click(&mut vcx, "theme-slot-light-trigger");
    assert!(
        app.update(&mut vcx, |app, _| app
            .active_settings()
            .unwrap()
            .menu
            .is_none()),
        "a disabled control opens no menu"
    );
    assert_eq!(cfg_theme(&app, &mut vcx), before);
}

/// A query reaches the slot rows through the same entries as every other row.
#[gpui::test]
fn queries_hit_the_slot_rows_and_keep_the_palette_keywords(cx: &mut TestAppContext) {
    let cfg = Config::default();
    for query in [
        "theme",
        "palette",
        "light theme",
        "dark theme",
        "theme_preset_dark",
    ] {
        let hits = shell::search_result_order(query, false, &cfg, ThemeView::default());
        assert!(
            hits.iter()
                .any(|e| e.title == L10nKey::SettingsLightThemeLabel
                    || e.title == L10nKey::SettingsDarkThemeLabel),
            "{query:?} reaches a theme slot row"
        );
    }
    for label in [
        t(L10nKey::SettingsLightThemeLabel),
        t(L10nKey::SettingsDarkThemeLabel),
    ] {
        assert!(
            row_matches_query(SettingsSection::Appearance, label, "palette"),
            "{label}: the palette keyword still marks the row"
        );
    }
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Appearance, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    set_query(&app, &mut vcx, "dark theme", false);
    assert!(
        has(&mut vcx, Slot::Dark.row()),
        "the search page carries the actual slot row"
    );
}

/// A dirty theme draft puts the slot Reset behind the unsaved-edits prompt:
/// Keep editing changes nothing, Discard drops the draft and then resets.
#[gpui::test]
fn slot_reset_asks_before_discarding_a_dirty_theme_draft(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a33.yaml");
    let (app, mut vcx) = harness(cx);
    let default_dark = Config::default().theme_preset_dark.clone();
    app.update_in(&mut vcx, |app, window, cx| {
        let mut theme = presets::all(cx)
            .into_iter()
            .find(|t| t.dark && t.id != default_dark)
            .expect("a dark builtin to copy");
        theme.id = "a33-test-theme".into();
        theme.path = Some(path.clone());
        presets::write_theme_file(&theme).unwrap();
        let mut themes = presets::all(cx);
        themes.push(theme);
        cx.set_global(presets::Themes(themes));
        let cfg = cx.global_mut::<Config>();
        cfg.theme_follow_system = false;
        cfg.theme_preset = "a33-test-theme".into();
        cfg.theme_preset_dark = "a33-test-theme".into();
        app.open_settings_section(SettingsSection::Appearance, window, cx);
        app.edit_active_theme(
            crate::ui::app::ThemeEdit::Accent,
            gpui::rgb(0x123456).into(),
            window,
            cx,
        );
        assert!(app.theme_draft_dirty());
        app.reset_settings_value(L10nKey::SettingsDarkThemeLabel, window, cx);
    });
    vcx.run_until_parked();
    assert!(
        vcx.has_pending_prompt(),
        "a dirty draft is asked about first"
    );
    assert_eq!(cfg_theme(&app, &mut vcx).1, "a33-test-theme");
    vcx.simulate_prompt_answer(t(L10nKey::SettingsKeepEditing));
    vcx.run_until_parked();
    assert_eq!(cfg_theme(&app, &mut vcx).1, "a33-test-theme");
    assert!(app.update(&mut vcx, |app, _| app.theme_draft_dirty()));

    app.update_in(&mut vcx, |app, window, cx| {
        app.reset_settings_value(L10nKey::SettingsDarkThemeLabel, window, cx)
    });
    vcx.run_until_parked();
    vcx.simulate_prompt_answer(t(L10nKey::EditorDiscard));
    vcx.run_until_parked();
    let (_, manual, _, dark) = cfg_theme(&app, &mut vcx);
    assert_eq!((manual, dark), (default_dark.clone(), default_dark));
    assert!(!app.update(&mut vcx, |app, _| app.theme_draft_dirty()));
}

/// Machine is session state: the row is searchable and has the row identity,
/// but there is nothing to reset and nothing to call modified.
#[gpui::test]
fn the_machine_row_is_searchable_and_never_resettable(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Agents, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1600.)));
    vcx.run_until_parked();
    assert!(
        has(&mut vcx, Slot::Machine.row()),
        "the page row carries the stable settings row id"
    );
    set_query(&app, &mut vcx, "machine", false);
    assert!(has(&mut vcx, Slot::Machine.result()));
    assert!(has(&mut vcx, Slot::Machine.row()));
    assert!(
        !has(&mut vcx, Slot::Machine.open_stub()),
        "the result is the page's own Machine row, not an Open setting stub"
    );
    assert!(!has(&mut vcx, Slot::Machine.reset()));
    assert!(!has(&mut vcx, Slot::Machine.tag()));
    set_query(&app, &mut vcx, "", true);
    assert!(
        !has(&mut vcx, Slot::Machine.result()),
        "a session choice is never in the modified list"
    );
    let entry = settings_search_entries()
        .iter()
        .find(|e| e.title == L10nKey::SettingsMachine)
        .expect("Machine is in the search index");
    assert_eq!(entry.config_key(), "", "no fake persistent key");
    assert!(!entry.modified(&Config::default(), ThemeView::default()));
}

/// Host form: the primary rests until Host is a valid address and then fills;
/// its label says what it does; a click while it rests saves nothing; Cancel
/// is a Link (the toolbar's Add host stays the only Secondary) and closes the
/// form.
#[gpui::test]
fn host_form_primary_follows_host_and_cancel_is_a_link(cx: &mut TestAppContext) {
    use crate::ui::i18n::{default_language_code, set_locale};
    set_locale("en");
    let _probe = paint_probe::scope();
    let (app, mut vcx) = harness(cx);
    let tk = vcx.update(|_, cx| super::kit::Tk::of(cx));
    let (rest, fill) = (paint_probe::hsla24(tk.k05), paint_probe::hsla24(tk.fg));
    let fresh = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        paint_probe::clear();
        app.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    };
    // The form shows exactly one Primary, and it is in `want` state.
    let primary_is = |want: u32, what: &str| {
        let primaries = paint_probe::all("settings.btn-primary");
        assert_eq!(primaries.len(), 1, "{what}: exactly one Primary is drawn");
        assert_eq!(primaries[0].fill, Some(want), "{what}");
    };
    let profiles = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        app.update(vcx, |_, cx| cx.global::<Config>().ssh_profiles.len())
    };
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Ssh, window, cx);
    });
    vcx.simulate_resize(size(px(960.), px(1200.)));
    fresh(&app, &mut vcx);
    let links_closed = paint_probe::all("settings.btn-link").len();
    assert!(
        paint_probe::all("settings.btn-primary").is_empty(),
        "no Primary without the form"
    );

    app.update_in(&mut vcx, |app, window, cx| app.add_new_profile(window, cx));
    fresh(&app, &mut vcx);
    primary_is(rest, "an empty host rests");
    assert_eq!(
        paint_probe::label("ssh-form-save").as_deref(),
        Some("Save host"),
        "a new host's primary says what it does"
    );
    assert_eq!(
        paint_probe::all("settings.btn-secondary").len(),
        1,
        "only the toolbar's Add host is a Secondary"
    );
    assert_eq!(
        paint_probe::all("settings.btn-link").len(),
        2,
        "with the form open the page's links are Import and Cancel (the list's own \
         links are not drawn); the closed page had {links_closed}"
    );
    let before = profiles(&app, &mut vcx);
    click(&mut vcx, "ssh-form-save");
    assert_eq!(
        profiles(&app, &mut vcx),
        before,
        "a blank click saves nothing"
    );
    assert!(
        app.update(&mut vcx, |app, _| app
            .active_settings()
            .unwrap()
            .ssh_form
            .is_some()),
        "and leaves the form open"
    );

    let (host, port) = vcx.update(|_, cx| {
        let f = app
            .read(cx)
            .active_settings()
            .unwrap()
            .ssh_form
            .as_ref()
            .unwrap();
        (f.host.clone(), f.port.clone())
    });
    let type_into = |vcx: &mut VisualTestContext,
                     input: &Entity<gpui_component::input::InputState>,
                     v: &str| {
        app.update_in(vcx, |_, window, cx| {
            input.update(cx, |i, cx| i.set_value(v, window, cx))
        });
        vcx.run_until_parked();
    };
    type_into(&mut vcx, &host, "box.example");
    fresh(&app, &mut vcx);
    primary_is(fill, "a valid host fills");
    type_into(&mut vcx, &port, "abc");
    fresh(&app, &mut vcx);
    primary_is(rest, "an invalid port rests");
    click(&mut vcx, "ssh-form-save");
    assert_eq!(
        profiles(&app, &mut vcx),
        before,
        "an invalid click saves nothing"
    );

    click(&mut vcx, "ssh-form-cancel");
    assert!(
        app.update(&mut vcx, |app, _| app
            .active_settings()
            .unwrap()
            .ssh_form
            .is_none()),
        "Cancel closes the form"
    );
    set_locale(default_language_code());
}
