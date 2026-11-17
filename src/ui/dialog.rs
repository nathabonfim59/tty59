//! Chrome for the small modal cards the app draws itself — the SSH sheet, the
//! worktree prompt — and the buttons and key hints the home page shares with
//! them.
//!
//! The workspace switcher is the reference: the same 12px card, 48px header
//! row with an `esc` cap, hairline-divided footer and faint keycaps. Each of
//! these cards used to spell its own padding, title weight and button variants
//! out a builder call at a time, and they had drifted — a 20px inset here, a
//! semibold title there, the accent-filled primary everywhere. One place now,
//! so two sheets on screen at once cannot disagree about what a dialog is.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FocusHandle, Hsla, KeyDownEvent, SharedString,
    Stateful, Window, div, prelude::*, px, rems,
};
use gpui_component::input::Input;
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use crate::ui::presets::Surface;
use crate::ui::right_panel::{HEADING, TAB_TEXT, TEXT};

/// The card's corner — the switcher's, a notch rounder than a menu: a dialog
/// is the one thing on screen while it is up.
pub(crate) const CARD_RADIUS: f32 = 12.;

/// The title row, the height of the switcher's search row, so a prompt that
/// opens where the switcher just closed puts its first line in the same place.
pub(crate) const HEADER_H: f32 = 48.;

/// The button row. A 28px button in it leaves 6px above and below, the same
/// fit as the switcher's footer and its New workspace button.
pub(crate) const FOOTER_H: f32 = 40.;

/// How far the title and the fields sit in from the card's edge — the
/// switcher's search-row inset, so the text columns of the two line up.
pub(crate) const INSET: f32 = 18.;

/// A text field: a borderless well on the faint fill, the sidebar search's
/// shape. The field's own `.small()` padding (8px) is what sets the text in.
pub(crate) const FIELD_H: f32 = 28.;

/// Space between a field's label and its well, and between one field and the
/// next. Four-pixel grid: tight inside a pair, a step wider between pairs.
const LABEL_GAP: f32 = 6.;
const FIELD_GAP: f32 = 14.;

/// A button: the field's height, and the design system's 6px corner — a notch
/// under the well's, so a button in a well-height row does not read as a second
/// input. The wells keep their own radius.
pub(crate) const BUTTON_H: f32 = 28.;
const BUTTON_RADIUS: f32 = 6.;
const BUTTON_PAD: f32 = 14.;

/// The panel-primary button's height, for the actions that sit in a panel, a
/// strip or a notice rather than in a card's footer. A button opts in by
/// asking for it: the dialog footer's [`BUTTON_H`] is unchanged.
pub(crate) const PANEL_BUTTON_H: f32 = 26.;

/// A key named in a hint: an 18px cap on the faint fill.
pub(crate) const KEYCAP: f32 = 18.;

/// The ladder a dialog's hovers are taken from. Cards sit on the popover
/// surface; the home page passes the window's own.
pub(crate) fn popover_rungs(cx: &App) -> Surface {
    cx.global::<crate::ui::presets::Surfaces>().popover
}

/// The card itself, opaque and occluding. Callers add the header, body and
/// footer as children, in that order.
///
/// `title` is what a screen reader announces the card as — a dialog, named
/// by the question it asks.
pub(crate) fn card(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    width: f32,
    cx: &App,
) -> Stateful<Div> {
    v_flex()
        .id(id)
        .role(gpui::Role::Dialog)
        .aria_label(title)
        .occlude()
        .w(px(width))
        .map(|panel| crate::ui::theme::floating_surface(panel, cx))
        .rounded(px(CARD_RADIUS))
        .overflow_hidden()
}

/// The title row: the title at body size in medium, and the `esc` cap on the
/// right that says how to back out. Every card here answers Escape.
pub(crate) fn header(title: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(HEADER_H))
        .pl(px(INSET))
        .pr(px(14.))
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(rems(TEXT))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title),
        )
        .child(keycap("esc", cx))
}

/// The card's content column.
pub(crate) fn body() -> Div {
    v_flex()
        .gap(px(FIELD_GAP))
        .px(px(INSET))
        .pt(px(16.))
        .pb(px(18.))
}

/// The button row: answers on the right, the action last, over a hairline.
pub(crate) fn footer(cx: &App) -> Div {
    h_flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap(px(6.))
        .h(px(FOOTER_H))
        .px(px(6.))
        .border_t_1()
        .border_color(cx.theme().border)
}

/// A field's name: the compact section-heading step, medium, secondary ink.
pub(crate) fn label(text: impl IntoElement, cx: &App) -> Div {
    div()
        .text_size(rems(HEADING))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// The fill of a well on a card: one rung up the card's own ladder.
///
/// Not the theme's `muted`. That one is a step off the *window*, and a card
/// sits on the popover surface, which a dark theme lifts to about the same
/// value — so in the dark every field on a dialog lost its shape and the
/// labels floated over nothing.
pub(crate) fn well_fill(cx: &App) -> Hsla {
    gpui::rgb(popover_rungs(cx).hover).into()
}

/// A text field in its well. The input loses its own border and fill — the
/// well is the shape — and keeps its padding, which is the text's inset.
pub(crate) fn field(input: Input, cx: &App) -> Div {
    h_flex()
        .items_center()
        .h(px(FIELD_H))
        .rounded(crate::ui::rounding::ROW_RADIUS)
        .bg(well_fill(cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(input.appearance(false).small()),
        )
}

/// A label over its field.
pub(crate) fn labelled(name: impl IntoElement, input: Input, cx: &App) -> Div {
    v_flex()
        .gap(px(LABEL_GAP))
        .child(label(name, cx))
        .child(field(input, cx))
}

/// A label over a control that is not a text field.
pub(crate) fn labelled_control(name: impl IntoElement, control: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .gap(px(LABEL_GAP))
        .child(label(name, cx))
        .child(control)
}

/// A dropdown's closed face, in the same well as a text field, so a form of
/// fields and choices still reads as one column.
pub(crate) fn select_well(
    id: impl Into<gpui::ElementId>,
    value: impl Into<gpui::SharedString>,
    cx: &App,
) -> gpui::Stateful<Div> {
    let tk = crate::ui::settings::kit::Tk::of(cx);
    h_flex()
        .id(id)
        .h(px(FIELD_H))
        .pl(px(10.))
        .pr(px(9.))
        .gap(px(8.))
        .items_center()
        .rounded(crate::ui::rounding::ROW_RADIUS)
        .bg(well_fill(cx))
        .cursor_pointer()
        .child(div().flex_1().min_w_0().truncate().child(value.into()))
        .child(crate::ui::settings::kit::chevron_down(&tk))
}

/// A block of read-only detail — a host and its key — set on the field's
/// fill, so it reads as a value to check rather than a line of prose.
pub(crate) fn well(cx: &App) -> Div {
    v_flex()
        .gap(px(4.))
        .px(px(10.))
        .py(px(8.))
        .rounded(crate::ui::rounding::ROW_RADIUS)
        .bg(well_fill(cx))
}

/// A key named in a hint: a small faint cap, never a button outline. The same
/// cap the switcher's header and footer draw.
pub(crate) fn keycap(label: impl Into<SharedString>, cx: &App) -> gpui::AnyElement {
    let label: SharedString = label.into();
    #[cfg(test)]
    let selector = format!("dialog-keycap-{label}");
    let ink = crate::ui::theme::chrome_ink(cx);
    let cap = div()
        .flex_shrink_0()
        .min_w(px(KEYCAP))
        .h(px(KEYCAP))
        .px(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(gpui::rgb(ink.keycap_fill))
        .text_size(rems(11. / 16.))
        .text_color(gpui::rgb(ink.keycap_ink))
        .child(label);
    #[cfg(test)]
    let mut cap = cap.debug_selector(move || selector);
    #[cfg(test)]
    crate::ui::paint_probe::note("dialog.keycap", &mut cap);
    cap.into_any_element()
}

/// A key combination as one cap per key — `⌘` `T`, never `⌘T` in one. The
/// way Search Everywhere and the shortcut editor spell a chord; the home page
/// and the switcher's footer had each packed theirs into a single cap.
pub(crate) fn chord(tokens: impl IntoIterator<Item = impl Into<SharedString>>, cx: &App) -> Div {
    h_flex()
        .flex_shrink_0()
        .gap(px(3.))
        .children(tokens.into_iter().map(|t| keycap(t, cx)))
}

/// What a button is asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    /// The one answer a card is for: an inverted neutral, the surface's ink as
    /// the fill — the Commit button's paint, not the accent. The accent is
    /// already spent on focus and on the rail; a dialog of blue buttons made
    /// every sheet look like a call to action.
    Primary,
    /// Everything else: the well's faint fill at rest, one rung up under the
    /// pointer. A bare word with no shape read as a link, not as the second
    /// answer to the question.
    Secondary,
    /// The action a card warns about. Red, and only on the one site that has
    /// earned it — overriding a changed host key.
    Danger,
    /// A destructive action that needs confirming but is not the card's main
    /// warning: no fill, a 1px danger border and danger ink, with an 8% danger
    /// wash under the pointer. The filled [`Tone::Danger`] stays reserved for
    /// the one answer a card exists to warn about.
    DangerOutline,
}

/// The `paint_probe` key a button's own paints are noted under, at rest or
/// under the pointer (test builds only; nothing here reaches a release build).
/// The destructive-outline keys are the ones its contrast test has always read;
/// Primary and Secondary are noted beside them so a consumer's sweep can read
/// the real hover fills. `Danger` has no note.
#[cfg(test)]
fn probe_key(tone: Tone, hover: bool) -> Option<&'static str> {
    match (tone, hover) {
        (Tone::DangerOutline, false) => Some("dialog.button.danger_outline"),
        (Tone::DangerOutline, true) => Some("dialog.button.danger_outline.hover"),
        (Tone::Primary, false) => Some("dialog.button.primary"),
        (Tone::Primary, true) => Some("dialog.button.primary.hover"),
        (Tone::Secondary, false) => Some("dialog.button.secondary"),
        (Tone::Secondary, true) => Some("dialog.button.secondary.hover"),
        (Tone::Danger, _) => None,
    }
}

/// A dialog button. The click handler is attached only while `enabled`: a
/// disabled button has to be inert, not merely grey.
///
/// Disabled, every tone sinks to the faint fill with secondary ink — the
/// same fall the Commit button takes — so it never advertises an action that
/// cannot run.
pub(crate) fn button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    tone: Tone,
    enabled: bool,
    rungs: Surface,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = cx.theme();
    let surface: Hsla = gpui::rgb(rungs.base).into();
    debug_assert!(
        tone != Tone::DangerOutline || rungs.base == popover_rungs(cx).base,
        "DangerOutline's ink is floored on the popover card only"
    );
    let (fill, hover, ink): (Option<Hsla>, Option<Hsla>, Hsla) = match (tone, enabled) {
        // The surface's own next rung, like a well: `muted` is a step off the
        // window, and on a dark popover it was the card's own colour.
        (_, false) => (
            Some(gpui::rgb(rungs.hover).into()),
            None,
            theme.muted_foreground,
        ),
        (Tone::Primary, true) => {
            let (fill, ink, hover) = primary_paints(rungs, cx);
            (Some(fill), Some(hover), ink)
        }
        (Tone::Danger, true) => {
            let red = theme.danger;
            (
                Some(red),
                Some(red.blend(surface.opacity(0.14))),
                theme.danger_foreground,
            )
        }
        // Ink and wash are the cached pair floored on the popover card, which
        // is the one surface this tone is built for.
        (Tone::DangerOutline, true) => {
            let cached = crate::ui::theme::danger_outline_ink(cx);
            (
                None,
                Some(gpui::rgb(cached.popover_wash).into()),
                gpui::rgb(cached.popover).into(),
            )
        }
        (Tone::Secondary, true) => (
            Some(gpui::rgb(rungs.hover).into()),
            Some(gpui::rgb(rungs.selected).into()),
            theme.foreground,
        ),
    };
    let el = h_flex()
        .id(id)
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(BUTTON_H))
        .px(px(BUTTON_PAD))
        .rounded(px(BUTTON_RADIUS))
        .text_size(rems(TAB_TEXT))
        .when(tone != Tone::Secondary, |b| {
            b.font_weight(gpui::FontWeight::MEDIUM)
        })
        .text_color(ink)
        .when_some(fill, |b, fill| b.bg(fill))
        .when(tone == Tone::DangerOutline && enabled, |b| {
            b.border_1().border_color(theme.danger)
        })
        .when_some(hover, |b, hover| {
            b.hover(move |s| {
                let s = s.bg(hover);
                #[cfg(test)]
                let mut s = s;
                #[cfg(test)]
                if let Some(key) = probe_key(tone, true) {
                    crate::ui::paint_probe::note(key, &mut s);
                }
                s
            })
        })
        .when(enabled, |b| b.cursor_pointer().on_click(on_click))
        .child(text.into());
    #[cfg(test)]
    let mut el = el;
    #[cfg(test)]
    if let Some(key) = probe_key(tone, false) {
        crate::ui::paint_probe::note(key, &mut el);
    }
    el
}

/// The Primary recipe's three colours on `rungs`: the theme's foreground as the
/// fill, the surface's cached label and the cached hover fill. Everything but
/// the fill was derived once, with the surface, when the theme was applied, so
/// this is three reads; [`button`] and [`primary_button`] both paint exactly
/// these.
fn primary_paints(rungs: Surface, cx: &App) -> (Hsla, Hsla, Hsla) {
    (
        cx.theme().foreground,
        gpui::rgb(rungs.primary.ink).into(),
        gpui::rgb(rungs.primary.hover).into(),
    )
}

/// The Primary recipe on the library's own [`Button`], for the panel, strip and
/// notice actions that are library buttons. The library keeps what it gives a
/// button — its keyed focus handle and tab stop, its button role and
/// accessible name, its disabled gate and its click handling — and the caller
/// keeps the id, the label and `on_click`; only the paint is the recipe's.
///
/// The library paints a custom colour as a 20% tint at rest and at 15% when
/// disabled, which is what made the old inverted button read as disabled. The
/// colours here go in through the library's custom variant, where its hover and
/// press paints are read (the one place a hover can be set: the library
/// installs its own and gpui refuses a second), and the opaque rest fill and
/// the disabled paint are put on the button's own style, which the library
/// applies after the variant. The same 26px height and 6px corner as
/// [`button`]'s panel form.
pub(crate) fn primary_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    rungs: Surface,
    cx: &App,
) -> gpui_component::button::Button {
    use gpui_component::Disableable as _;
    use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};

    let (fill, ink, hover) = primary_paints(rungs, cx);
    let variant = ButtonCustomVariant::new(cx)
        .color(fill)
        .foreground(ink)
        .hover(hover)
        .active(hover);
    let mut button = Button::new(id)
        .label(label)
        .custom(variant)
        .small()
        .rounded(px(BUTTON_RADIUS))
        .disabled(!enabled)
        .h(px(PANEL_BUTTON_H))
        .px(px(BUTTON_PAD));
    button = if enabled {
        button.bg(fill).text_color(ink)
    } else {
        button
            .bg(Hsla::from(gpui::rgb(rungs.hover)))
            .text_color(cx.theme().muted_foreground)
    };
    #[cfg(test)]
    {
        crate::ui::paint_probe::note("dialog.panel_primary", &mut button);
        if enabled {
            let mut wash = gpui::StyleRefinement::default().bg(hover);
            crate::ui::paint_probe::note("dialog.panel_primary.hover", &mut wash);
        }
    }
    button
}

/// The widest an overflow tooltip's visible card is: the hover maximum.
const TOOLTIP_MAX_W: f32 = 420.;

/// What an overflow tooltip leaves of the window's width, its outer margins
/// included.
const TOOLTIP_GUTTER: f32 = 16.;

/// An opt-in tooltip for text that may be longer than the window: the plain
/// text, whole, in the card every tooltip is, bounded by the window. Nothing
/// that does not call it changes; the shared tooltip's defaults are untouched.
///
/// The card is at most the hover maximum, and with its outer margin (`m_3`,
/// 0.75 rem a side) at most the window less a gutter: read when the tooltip is
/// shown, so a resized window is honored. Text only wraps at a width it is
/// given and reports its whole unwrapped width as its smallest, so the card's
/// cap alone leaves the content wider than the card: the content gets the
/// card's inner cap, its border box less its padding (`px_2p5`, 0.625 rem a
/// side) and border.
///
/// `selector` names the content for a test to find where the real tooltip put
/// it; it is only attached in test builds.
pub(crate) fn bounded_text_tooltip(
    text: SharedString,
    selector: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> gpui::AnyView {
    let rem = window.rem_size().as_f32();
    let margin = 0.75 * rem;
    let limit = TOOLTIP_MAX_W
        .min(window.viewport_size().width.as_f32() - TOOLTIP_GUTTER - 2. * margin)
        .max(0.);
    let content_limit = (limit - 2. * (0.625 * rem + 1.)).max(0.);
    gpui_component::tooltip::Tooltip::element(move |_, _| {
        div()
            .max_w(px(content_limit))
            .whitespace_normal()
            .when(cfg!(test), |d| d.debug_selector(|| selector.to_string()))
            .child(text.clone())
    })
    .max_w(px(limit))
    .whitespace_normal()
    .build(window, cx)
}

/// What a footer button is to the keyboard: whether it is enabled, its own
/// stable handle, and the handles of the enabled buttons of the same row in
/// visual order, which the arrow keys move along. The handles are owned by
/// the caller's state, never made while drawing; `enabled` is read off the
/// same value that paints the button and gates its click, focus and ring.
pub(crate) struct ButtonFocus<'a> {
    pub enabled: bool,
    pub handle: &'a FocusHandle,
    pub row: &'a [FocusHandle],
}

/// A [`button`] a keyboard can reach: the same paint, and the same pointer
/// path, but an enabled one is a tab stop on `focus.handle`, answers a fresh
/// Enter or Space press by running `on_activate` once, moves focus along the
/// row on Left and Right (stopping at the ends), and wears the shared focus
/// ring while focused.
///
/// A disabled one is [`button`]'s inert button as it was: no handle, no key
/// listener, no ring, and so no stop in the Tab order.
///
/// The press is taken on key-down, and only a first, unmodified one: a
/// repeat of a held key does nothing, and the toolkit's own keyboard click,
/// which fires on key-*up*, is dropped. The release of a key that was
/// already down when a sheet appeared cannot then answer it.
pub(crate) fn focus_button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    tone: Tone,
    rungs: Surface,
    focus: &ButtonFocus<'_>,
    cx: &App,
    on_activate: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let enabled = focus.enabled;
    let id: ElementId = id.into();
    // Tests find the button by its id, disabled or not: `dialog-button-<id>`.
    #[cfg(test)]
    let selector = format!("dialog-button-{id}");
    let on_activate = Rc::new(on_activate);
    let on_click = {
        let on_activate = on_activate.clone();
        move |ev: &ClickEvent, window: &mut Window, cx: &mut App| {
            if !ev.is_keyboard() {
                on_activate(window, cx);
            }
        }
    };
    let el = button(id, text, tone, enabled, rungs, cx, on_click);
    #[cfg(test)]
    let el = gpui::InteractiveElement::debug_selector(el, move || selector);
    if !enabled {
        return el.into_any_element();
    }
    let own = focus.handle.clone();
    let row = focus.row.to_vec();
    let el = el
        .track_focus(&focus.handle.clone().tab_stop(true))
        .on_key_down({
            let own = own.clone();
            move |ev: &KeyDownEvent, window: &mut Window, cx: &mut App| {
                if ev.keystroke.modifiers.modified() {
                    return;
                }
                match ev.keystroke.key.as_str() {
                    "enter" | "space" => {
                        cx.stop_propagation();
                        if !ev.is_held {
                            on_activate(window, cx);
                        }
                    }
                    key @ ("left" | "right") => {
                        cx.stop_propagation();
                        if let Some(at) = row.iter().position(|h| *h == own) {
                            let to = match key {
                                "left" => at.saturating_sub(1),
                                _ => (at + 1).min(row.len() - 1),
                            };
                            if to != at {
                                window.focus(&row[to], cx);
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    crate::ui::theme::FocusRing::new(
        &own,
        crate::ui::theme::RingPlacement::Outside,
        px(BUTTON_RADIUS),
        el,
    )
    .into_any_element()
}

/// What the real-render contrast tests of the switcher, Search Everywhere, the
/// pane grip, the port chips and the forward rows share: the themes they run
/// under, how a test window switches to one, and the floors the external
/// contract states (text 4.5:1, marks 3:1 — not read off any private constant).
#[cfg(test)]
pub(crate) mod test_themes {
    use gpui::VisualTestContext;

    use crate::ui::presets::{self, Theme};

    pub(crate) const TEXT_FLOOR: f32 = 4.5;
    pub(crate) const MARK_FLOOR: f32 = 3.0;

    /// The built-ins, a spread of the generated user-style palettes and the
    /// crossover ones whose chrome fills invert.
    pub(crate) fn themes() -> Vec<Theme> {
        let mut themes = presets::builtins();
        let grid = presets::generated_theme_grid();
        themes.extend(grid.iter().cloned().step_by(37));
        themes.extend(
            grid.into_iter()
                .filter(|t| t.background_color() == 0x777777)
                .step_by(11),
        );
        themes
    }

    /// Switches the window to `theme` the way a theme change does: by
    /// `apply_theme`, which is where every cached ink is derived.
    pub(crate) fn apply(vcx: &mut VisualTestContext, theme: &Theme) {
        vcx.update(|window, cx| {
            let mut registry = presets::all(cx);
            registry.retain(|t| t.id != theme.id);
            registry.push(theme.clone());
            cx.set_global(presets::Themes(registry));
            let mut config = cx.global::<crate::core::config::Config>().clone();
            config.theme_follow_system = false;
            config.theme_preset = theme.id.clone();
            cx.set_global(config);
            crate::ui::theme::apply_theme(Some(window), cx);
        });
    }

    /// Collects every ink that misses its floor, so one run names all the
    /// counterexamples rather than the first.
    #[derive(Default)]
    pub(crate) struct Misses(Vec<String>);

    impl Misses {
        pub(crate) fn floor(&mut self, theme: &str, what: &str, ink: u32, fill: u32, floor: f32) {
            let got = presets::contrast(ink, fill);
            if got < floor {
                self.0.push(format!(
                    "{theme}: {what} is {got:.2}:1 ({ink:06x} on {fill:06x}), floor {floor}"
                ));
            }
        }

        pub(crate) fn assert_none(self, what: &str) {
            assert!(
                self.0.is_empty(),
                "{what}: {} misses\n{}",
                self.0.len(),
                self.0.join("\n")
            );
        }
    }
}

/// A property of the recipe, not of any screen: built on each real theme, the
/// destructive-outline button's ink and wash clear the text floor and its
/// border the component floor against the popover surface. Whether a given
/// control uses the recipe is that consumer's own witness.
#[cfg(test)]
mod danger_outline_tests {
    use gpui::TestAppContext;

    use super::test_themes::{self as themes, MARK_FLOOR, Misses, TEXT_FLOOR};
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::paint_probe::{self as probe, hsla24};

    #[gpui::test]
    fn danger_outline_clears_the_text_and_component_floors(cx: &mut TestAppContext) {
        let (_app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let _probe = probe::scope();
        let mut misses = Misses::default();
        for theme in &themes::themes() {
            themes::apply(&mut vcx, theme);
            probe::clear();
            let surface = vcx.update(|_, cx| {
                let rungs = popover_rungs(cx);
                let _ = button(
                    "rest",
                    "Revert",
                    Tone::DangerOutline,
                    true,
                    rungs,
                    cx,
                    |_, _, _| {},
                );
                let _ = button(
                    "off",
                    "Revert",
                    Tone::DangerOutline,
                    false,
                    rungs,
                    cx,
                    |_, _, _| {},
                );
                rungs.base
            });
            let id = theme.id.as_str();
            let notes = probe::all("dialog.button.danger_outline");
            assert_eq!(notes.len(), 2, "{id}: the enabled and the disabled button");
            let (rest, off) = (notes[0], notes[1]);
            assert_eq!(rest.fill, None, "{id}: no fill at rest");
            assert_eq!(off.border, None, "{id}: a disabled one draws no outline");
            // The wash is what the enabled button paints under the pointer.
            let washes = probe::all("dialog.button.danger_outline.hover");
            assert_eq!(washes.len(), 1, "{id}: one wash, for the enabled button");
            let wash = washes[0].fill.expect("the wash");
            let ink = rest.text.expect("the ink");
            let border = rest.border.expect("the 1px outline");
            misses.floor(id, "ink at rest", ink, surface, TEXT_FLOOR);
            misses.floor(id, "ink under the pointer", ink, wash, TEXT_FLOOR);
            misses.floor(id, "outline", border, surface, MARK_FLOOR);
            // The wash is 8% of the danger ink over the surface, worked out
            // here channel by channel rather than by the recipe's own helper.
            let danger = vcx.update(|_, cx| hsla24(cx.theme().danger));
            for shift in [16, 8, 0] {
                let channel = |rgb: u32| ((rgb >> shift) & 0xff) as f32;
                let expected = channel(surface) * 0.92 + channel(danger) * 0.08;
                assert!(
                    (channel(wash) - expected).abs() <= 1.0,
                    "{id}: wash channel {shift} is {} where 8% danger over the surface is {expected:.1}",
                    channel(wash),
                );
            }
        }
        misses.assert_none("danger outline contrast");
    }
}

/// The Primary button's contrast, measured on what `button` actually builds in
/// every theme the app can be given and on every surface its callers pass:
/// the popover card (the sheets, the switcher form), the window (the editor's
/// conflict bar) and the sidebar and rail ladders (the panel actions and the
/// panel forms).
///
/// The contrast here is computed independently of `presets::contrast`: a
/// recipe that shares the helper with its own check would pass for the wrong
/// reason.
#[cfg(test)]
mod primary_contrast_tests {
    use gpui::TestAppContext;

    use super::test_themes::{self as themes, TEXT_FLOOR};
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::paint_probe as probe;
    use crate::ui::presets::{self, Theme};

    /// WCAG 2.x relative luminance and contrast ratio, from the definition.
    fn wcag(a: u32, b: u32) -> f32 {
        fn lin(v: u32) -> f64 {
            let s = v as f64 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        fn lum(c: u32) -> f64 {
            0.2126 * lin(c >> 16 & 0xff) + 0.7152 * lin(c >> 8 & 0xff) + 0.0722 * lin(c & 0xff)
        }
        let (la, lb) = (lum(a), lum(b));
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        ((hi + 0.05) / (lo + 0.05)) as f32
    }

    /// The built-ins and the whole generated grid the B3b sweep ran over
    /// (864 generated and the built-ins: 877 in all).
    fn corpus() -> Vec<Theme> {
        let mut all = presets::builtins();
        let grid = presets::generated_theme_grid();
        all.extend(grid);
        // The report's 877: the generated grid and the built-ins together.
        assert_eq!(all.len(), 877, "the corpus the report counted");
        all
    }

    /// What one Primary button's style carries on `rungs`: the label and the
    /// fill at rest, always, and the fill under the pointer where it has one.
    /// This is the recipe's resolved style as the probe notes it when the
    /// button is built, not a mounted render: the caller tests supply those.
    struct Paints {
        ink: u32,
        rest: u32,
        hover: Option<u32>,
    }

    fn primary_on(rungs: Surface, enabled: bool, cx: &App) -> Paints {
        probe::clear();
        let _ = button(
            "p",
            "Reconnect",
            Tone::Primary,
            enabled,
            rungs,
            cx,
            |_, _, _| {},
        );
        let rest = probe::all("dialog.button.primary");
        let hover = probe::all("dialog.button.primary.hover");
        assert_eq!(rest.len(), 1, "one button, one rest note");
        assert_eq!(
            hover.len(),
            enabled as usize,
            "a hover paint for an enabled button only"
        );
        let hover = match hover.first() {
            Some(h) => Some(h.fill.expect("a hover note carries its fill")),
            None => None,
        };
        Paints {
            ink: rest[0].text.expect("a button always has an ink"),
            rest: rest[0].fill.expect("a button always has a rest fill"),
            hover,
        }
    }

    fn enabled_hover(p: &Paints) -> u32 {
        p.hover.expect("an enabled Primary has a hover paint")
    }

    fn surfaces(cx: &App) -> [(&'static str, Surface); 4] {
        let sf = cx.global::<presets::Surfaces>();
        [
            ("window", sf.window),
            ("sidebar", sf.sidebar),
            ("rail", sf.rail),
            ("popover", sf.popover),
        ]
    }

    /// One sweep over every theme and surface: the label clears the text floor
    /// on the rest fill and on the hover fill, measured on the 8-bit values
    /// the button's style carries, not on what a recipe says it authored. The
    /// rest and hover misses are both counted, so one run names every theme
    /// and surface that misses either, and by how much.
    #[gpui::test]
    fn primary_clears_the_text_floor_at_rest_and_under_the_pointer_everywhere(
        cx: &mut TestAppContext,
    ) {
        let (_app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let _probe = probe::scope();
        let mut rest_misses = Vec::new();
        let mut hover_misses = Vec::new();
        let mut rest_themes = std::collections::BTreeSet::new();
        let mut hover_themes = std::collections::BTreeSet::new();
        let mut cells = 0;
        for theme in &corpus() {
            themes::apply(&mut vcx, theme);
            vcx.update(|_, cx| {
                for (name, rungs) in surfaces(cx) {
                    cells += 1;
                    let p = primary_on(rungs, true, cx);
                    let rest = wcag(p.ink, p.rest);
                    if rest < TEXT_FLOOR {
                        rest_themes.insert(theme.id.to_string());
                        rest_misses.push(format!(
                            "{} on {name}: rest {:06x} on {:06x} is {rest:.2}:1",
                            theme.id, p.ink, p.rest
                        ));
                    }
                    let hover = enabled_hover(&p);
                    let under = wcag(p.ink, hover);
                    if under < TEXT_FLOOR {
                        hover_themes.insert(theme.id.to_string());
                        hover_misses.push(format!(
                            "{} on {name}: hover {:06x} on {hover:06x} is {under:.2}:1",
                            theme.id, p.ink
                        ));
                    }
                }
            });
        }
        eprintln!(
            "{cells} theme/surface cells: rest misses {} in {} themes, hover misses {} in {} themes",
            rest_misses.len(),
            rest_themes.len(),
            hover_misses.len(),
            hover_themes.len()
        );
        assert!(
            rest_misses.is_empty() && hover_misses.is_empty(),
            "{cells} cells: {} rest misses in {} themes, {} hover misses in {} themes\n{}\n{}",
            rest_misses.len(),
            rest_themes.len(),
            hover_misses.len(),
            hover_themes.len(),
            rest_misses.join("\n"),
            hover_misses.join("\n")
        );
    }

    /// What the correction must not touch. The fill is the theme's foreground
    /// everywhere, corrected or not. Where the authored rest (the foreground
    /// fill under the surface as its label) already clears the floor, decided
    /// here from the two authored colours alone, the label is that surface
    /// exactly, and so is a hover whose authored blend (worked out here
    /// channel by channel) also clears it. Where the authored rest misses, the
    /// label is the only thing that changes. Misses are collected, not thrown
    /// at the first, so a repaired colour never decides what was preserved.
    #[gpui::test]
    fn an_authored_primary_that_passes_is_painted_exactly_and_a_failing_rest_moves_only_its_label(
        cx: &mut TestAppContext,
    ) {
        let (_app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let _probe = probe::scope();
        let mut misses = Vec::new();
        let (mut passing, mut failing, mut kept_hover) = (0, 0, 0);
        for theme in &corpus() {
            themes::apply(&mut vcx, theme);
            vcx.update(|_, cx| {
                for (name, rungs) in surfaces(cx) {
                    let case = format!("{} on {name}", theme.id);
                    let (fg, base) = (
                        probe::hsla24(cx.theme().foreground),
                        probe::hsla24(gpui::rgb(rungs.base).into()),
                    );
                    let p = primary_on(rungs, true, cx);
                    if p.rest != fg {
                        misses.push(format!(
                            "{case}: the fill moved from {fg:06x} to {:06x}",
                            p.rest
                        ));
                    }
                    // What the recipe has always painted, as the app's own
                    // 8-bit values: the foreground as the fill, the surface as
                    // the label, and the foreground with 14% of the surface
                    // over it as the hover, blended by gpui and quantized the
                    // way the probe reads every paint.
                    let surface: gpui::Hsla = gpui::rgb(rungs.base).into();
                    let authored_hover = probe::hsla24(cx.theme().foreground.blend(surface.opacity(0.14)));
                    let authored_rest = wcag(base, fg);
                    if authored_rest >= TEXT_FLOOR {
                        // Every passing rest is painted exactly as authored.
                        passing += 1;
                        if p.ink != base {
                            misses.push(format!(
                                "{case}: a passing rest ({authored_rest:.2}:1) had its label moved from {base:06x} to {:06x}",
                                p.ink
                            ));
                        }
                        // And so is its hover wherever that clears the floor
                        // on the (unchanged) label.
                        if wcag(base, authored_hover) >= TEXT_FLOOR {
                            kept_hover += 1;
                            let hover = enabled_hover(&p);
                            if hover != authored_hover {
                                misses.push(format!(
                                    "{case}: a passing authored hover {authored_hover:06x} moved to {hover:06x}"
                                ));
                            }
                        }
                    } else {
                        // Every failing rest changes its label and nothing else.
                        failing += 1;
                        if p.ink == base {
                            misses.push(format!(
                                "{case}: a failing rest ({authored_rest:.2}:1) kept its label {base:06x}"
                            ));
                        }
                    }
                }
            });
        }
        eprintln!(
            "{passing} passing rests, {failing} failing rests, {kept_hover} authored hovers that pass"
        );
        assert!(
            passing > 0 && failing > 0 && kept_hover > 0,
            "a vacuous sweep"
        );
        assert!(
            misses.is_empty(),
            "{} misses\n{}",
            misses.len(),
            misses.join("\n")
        );
    }

    /// The colours of the walk from `a` to `b` in 256 steps, each rounded to
    /// 8 bits per channel: the independent search space for a repaired hover,
    /// walked by the test itself rather than asked of the recipe.
    fn walk(a: u32, b: u32) -> Vec<u32> {
        (0..=255u32)
            .map(|k| {
                let t = k as f32 / 255.;
                let ch = |s: u32| {
                    let (x, y) = (((a >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
                    (x + (y - x) * t).round() as u32
                };
                ch(16) << 16 | ch(8) << 8 | ch(0)
            })
            .collect()
    }

    /// What the hover of a Primary is, for every theme and surface, judged from
    /// the authored colours and an independent walk, never from the recipe.
    ///
    /// - KEPT: the authored hover (the fill with 14% of the surface over it)
    ///   clears 4.5 against the final label: painted exactly as authored, with
    ///   no demand on how far it sits from the rest fill.
    /// - TOWARD: it does not, and some point on the walk from it to the rest
    ///   fill clears 4.5 against the label AND sits at least a panel hover's
    ///   step (1.07) from the rest fill: the painted hover is on that walk, past
    ///   neither floor, no further from the authored hover than that interval.
    /// - AWAY: no such point; some point on the walk from the rest fill away from
    ///   the label (toward white or black, whichever has more contrast with it)
    ///   does: the painted hover is the nearest such point to the rest fill.
    /// - EXCEPTION: neither walk has a qualifying point: the painted hover is
    ///   the rest fill. The count and the list go in the evidence.
    #[gpui::test]
    fn a_repaired_primary_hover_is_a_visible_step_from_its_rest_in_every_theme(
        cx: &mut TestAppContext,
    ) {
        const STEP: f32 = 1.07;
        let (_app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let _probe = probe::scope();
        let mut misses = Vec::new();
        let mut exceptions = Vec::new();
        let (mut kept, mut toward, mut away) = (0, 0, 0);
        for theme in &corpus() {
            themes::apply(&mut vcx, theme);
            vcx.update(|_, cx| {
                for (name, rungs) in surfaces(cx) {
                    let case = format!("{} on {name}", theme.id);
                    let p = primary_on(rungs, true, cx);
                    let (fill, ink, hover) = (p.rest, p.ink, enabled_hover(&p));
                    let surface: gpui::Hsla = gpui::rgb(rungs.base).into();
                    let authored =
                        probe::hsla24(cx.theme().foreground.blend(surface.opacity(0.14)));
                    if wcag(ink, authored) >= TEXT_FLOOR {
                        kept += 1;
                        if hover != authored {
                            misses.push(format!(
                                "{case}: a KEPT hover {authored:06x} was painted {hover:06x}"
                            ));
                        }
                        continue;
                    }
                    let qualifies = |c: u32| wcag(ink, c) >= TEXT_FLOOR && wcag(c, fill) >= STEP;
                    let toward_path = walk(authored, fill);
                    if let Some(first) = toward_path.iter().position(|&c| qualifies(c)) {
                        toward += 1;
                        if !qualifies(hover) {
                            misses.push(format!(
                                "{case}: the repaired hover {hover:06x} is {:.3}:1 against its rest {fill:06x} and {:.2}:1 against the label {ink:06x}: a qualifying point exists on the way back to the rest at step {first}",
                                wcag(hover, fill),
                                wcag(ink, hover)
                            ));
                        } else if !toward_path.contains(&hover) {
                            misses.push(format!(
                                "{case}: the repaired hover {hover:06x} is not on the walk from the authored {authored:06x} to the rest {fill:06x}"
                            ));
                        }
                        continue;
                    }
                    let pole = if wcag(0xffffff, ink) >= wcag(0x000000, ink) {
                        0xffffff
                    } else {
                        0x000000
                    };
                    let away_path = walk(fill, pole);
                    if let Some(first) = away_path.iter().position(|&c| qualifies(c)) {
                        away += 1;
                        let nearest = away_path[first];
                        let step = away_path.iter().position(|&c| c == hover);
                        if !qualifies(hover) {
                            misses.push(format!(
                                "{case}: the AWAY hover {hover:06x} is {:.3}:1 against its rest {fill:06x} and {:.2}:1 against the label {ink:06x}; the nearest qualifying point is {nearest:06x}",
                                wcag(hover, fill),
                                wcag(ink, hover)
                            ));
                        } else if step.map_or(true, |s| s > first + 2) {
                            misses.push(format!(
                                "{case}: the AWAY hover {hover:06x} is not the nearest qualifying point {nearest:06x} (step {first}) on the walk from the rest {fill:06x} toward {pole:06x}"
                            ));
                        }
                        continue;
                    }
                    exceptions.push(format!("{case}: fill {fill:06x}, label {ink:06x}"));
                    if hover != fill {
                        misses.push(format!(
                            "{case}: no direction has a qualifying point, so the hover must be the rest {fill:06x}, not {hover:06x}"
                        ));
                    }
                }
            });
        }
        eprintln!(
            "hover classes: {kept} kept, {toward} toward, {away} away, {} exceptions",
            exceptions.len()
        );
        for e in &exceptions {
            eprintln!("hover exception: {e}");
        }
        assert!(
            misses.is_empty(),
            "{kept} kept, {toward} toward, {away} away, {} exceptions; {} misses\n{}",
            exceptions.len(),
            misses.len(),
            misses.join("\n")
        );
    }

    /// The library button in the Primary recipe paints what `button` paints:
    /// the foreground as an opaque rest fill (not the library's 20% tint), the
    /// cached label, the cached hover, and the neutral rung under the muted ink
    /// with no hover when disabled. The recipe has one source, so the two
    /// builders cannot drift.
    #[gpui::test]
    fn the_library_primary_button_paints_the_same_recipe(cx: &mut TestAppContext) {
        let (_app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let _probe = probe::scope();
        let mut misses = Vec::new();
        for theme in &corpus() {
            themes::apply(&mut vcx, theme);
            vcx.update(|_, cx| {
                let muted = probe::hsla24(cx.theme().muted_foreground);
                for (name, rungs) in surfaces(cx) {
                    let case = format!("{} on {name}", theme.id);
                    let dialog = primary_on(rungs, true, cx);
                    probe::clear();
                    let _ = primary_button("p", "Go", true, rungs, cx);
                    let (rest, wash) = (
                        probe::all("dialog.panel_primary"),
                        probe::all("dialog.panel_primary.hover"),
                    );
                    match (rest.first(), wash.first()) {
                        (Some(rest), Some(wash)) => {
                            if rest.fill != Some(dialog.rest) {
                                misses.push(format!(
                                    "{case}: the library button's rest fill is {:?}, not the recipe's {:06x}",
                                    rest.fill, dialog.rest
                                ));
                            }
                            if rest.text != Some(dialog.ink) {
                                misses.push(format!(
                                    "{case}: the library button's label is {:?}, not {:06x}",
                                    rest.text, dialog.ink
                                ));
                            }
                            if wash.fill != dialog.hover {
                                misses.push(format!(
                                    "{case}: the library button's hover is {:?}, not {:?}",
                                    wash.fill, dialog.hover
                                ));
                            }
                        }
                        _ => misses.push(format!("{case}: an enabled button noted no paints")),
                    }

                    probe::clear();
                    let _ = primary_button("p", "Go", false, rungs, cx);
                    let off = probe::all("dialog.panel_primary");
                    match off.first() {
                        Some(off) => {
                            if off.fill != Some(rungs.hover) || off.text != Some(muted) {
                                misses.push(format!(
                                    "{case}: a disabled library button is {:?} on {:?}, not the neutral rung under the muted ink",
                                    off.text, off.fill
                                ));
                            }
                        }
                        None => misses.push(format!("{case}: a disabled button noted no paints")),
                    }
                    if !probe::all("dialog.panel_primary.hover").is_empty() {
                        misses.push(format!("{case}: a disabled library button has a hover"));
                    }
                }
            });
        }
        assert!(
            misses.is_empty(),
            "{} misses\n{}",
            misses.len(),
            misses.join("\n")
        );
    }

    /// A disabled Primary is inert paint, in every theme and on every surface:
    /// the surface's next rung under the muted ink, and no hover. Separate
    /// from the contrast sweep so a miss there cannot hide it.
    #[gpui::test]
    fn a_disabled_primary_is_a_neutral_rung_with_no_hover(cx: &mut TestAppContext) {
        let (_app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let _probe = probe::scope();
        let mut misses = Vec::new();
        for theme in &corpus() {
            themes::apply(&mut vcx, theme);
            vcx.update(|_, cx| {
                let muted = probe::hsla24(cx.theme().muted_foreground);
                for (name, rungs) in surfaces(cx) {
                    let off = primary_on(rungs, false, cx);
                    let case = format!("{} on {name}", theme.id);
                    if off.rest != rungs.hover {
                        misses.push(format!(
                            "{case}: the fill is {:06x}, not the surface's next rung {:06x}",
                            off.rest, rungs.hover
                        ));
                    }
                    if off.ink != muted {
                        misses.push(format!(
                            "{case}: the label is {:06x}, not the muted ink {muted:06x}",
                            off.ink
                        ));
                    }
                    if off.hover.is_some() {
                        misses.push(format!("{case}: a disabled Primary has a hover paint"));
                    }
                }
            });
        }
        assert!(
            misses.is_empty(),
            "{} misses\n{}",
            misses.len(),
            misses.join("\n")
        );
    }
}
