use std::path::PathBuf;

use gpui::{App, Global, Hsla};
use serde::Deserialize;
use tty59_vt::model::Rgb;

use crate::terminal::palette::ActivePalette;

#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    Solid(u32),
    Vertical { top: u32, bottom: u32 },
    Horizontal { left: u32, right: u32 },
}

impl Fill {
    pub fn color(&self) -> u32 {
        match *self {
            Fill::Solid(c) => c,
            Fill::Vertical { top, .. } => top,
            Fill::Horizontal { left, .. } => left,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub path: PathBuf,
    pub opacity: f32,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub dark: bool,
    pub background: Fill,
    pub foreground: u32,
    pub accent: u32,
    pub caret: Option<u32>,
    pub selection: Option<u32>,
    pub opacity: Option<f32>,
    pub blur: bool,
    pub image: Option<Image>,
    pub ansi16: [(u8, u8, u8); 16],
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Neutrals {
    pub background: u32,
    pub foreground: u32,
    pub border: u32,
    pub divider: u32,
    pub secondary: u32,
    pub muted: u32,
    pub muted_foreground: u32,
    pub popover: u32,
    pub caret: u32,
    pub selection: u32,
    pub sidebar: u32,
    pub sidebar_fg: u32,
    /// The left rail's own fill: a small step off the window toward the
    /// foreground, so the tab column reads as navigation beside the work.
    /// The right panel keeps `sidebar` (the window fill).
    pub rail: u32,
    /// The tab title ink on `rail`, floored the way `sidebar_fg` is.
    pub rail_fg: u32,
    pub accent: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Semantic {
    pub ink: u32,
    pub fill: u32,
    pub on_fill: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Semantics {
    pub danger: Semantic,
    pub warning: Semantic,
    pub success: Semantic,
    pub info: Semantic,
    pub link: Semantic,
}

impl Global for Semantics {}

impl Semantics {
    /// Maps core agent status onto the active theme's existing semantic roles
    /// so `tty59-core` stays free of UI theme dependencies.
    pub(crate) fn for_agent_status(
        &self,
        status: crate::core::cli_agent::AgentStatus,
    ) -> Option<Semantic> {
        match status {
            crate::core::cli_agent::AgentStatus::Idle => None,
            crate::core::cli_agent::AgentStatus::Working => Some(self.info),
            crate::core::cli_agent::AgentStatus::Waiting => Some(self.warning),
            crate::core::cli_agent::AgentStatus::Done => Some(self.success),
        }
    }
}

/// The settings page's secondary text and off-state switch track, cached per
/// theme by `apply_theme`.
#[derive(Debug, Clone, Copy)]
pub struct SettingsInk {
    pub sub: u32,
    pub switch_off: u32,
    /// The search glass and its like: a mark, held to 3:1 on the settings
    /// fills it sits on, from a seed at half the foreground's strength.
    pub glyph: u32,
    /// The semantic text inks (an error, Unsaved, Live) floored on the
    /// settings fills they are painted on, which are lower-contrast than the
    /// bases `Semantics` is floored on: a hovered row, a dropdown row.
    pub danger: u32,
    pub warn_text: u32,
    pub ok: u32,
    /// The warning dot and ring, a mark: the semantic warning fill floored at
    /// 3:1 on the page and on the row wash a shortcut conflict sits on.
    pub warn: u32,
    /// Signed lightness a light control sits off the page, and a dark
    /// popover sits off it: the `settings_wash` amounts, in the direction
    /// the shared pole puts them.
    pub raise: f32,
    pub menu_lift: f32,
}

impl Global for SettingsInk {}

/// Switcher, search overlay and pane-grip ink, cached per theme by
/// `apply_theme`; each floored on the fills it is painted on.
#[derive(Debug, Clone, Copy)]
pub struct ChromeInk {
    /// Meta, badge and slot text on the selected popover row.
    pub selected_meta: u32,
    /// An unlit name on the popover's resting, hovered and selected rows.
    pub quiet_name: u32,
    /// An avatar's initial on `Neutrals::secondary`.
    pub avatar_ink: u32,
    /// A keycap's fill, a neutral step off the popover.
    pub keycap_fill: u32,
    /// A keycap's label on `keycap_fill`.
    pub keycap_ink: u32,
    /// An idle pane grip on the window (a mark: 3:1).
    pub grip_idle: u32,
    /// A search result's title on the popover's resting, hovered and selected
    /// rows: the foreground at 92%, floored.
    pub search_title: u32,
    /// A panel row's title on the sidebar's and the rail's resting, hovered
    /// and selected rows: the authored foreground, floored.
    pub panel_title: u32,
    /// A port chip's text on its translucent fill over a sidebar or window
    /// row, resting and hovered: the authored foreground, floored.
    pub port_chip_ink: u32,
    /// The same chip's text while its row is hovered: the theme's link ink,
    /// floored (`ThemeColor::accent` is a fill and cannot be text).
    pub port_chip_hover_ink: u32,
    /// An unpicked switcher row's title on the popover's resting and hovered
    /// rows: the authored foreground, floored.
    pub unpicked_title: u32,
    /// An unpicked switcher row's meta on the same rows: the theme's muted
    /// foreground, floored.
    pub unpicked_meta: u32,
}

/// How far a port chip's wash steps off the row it sits on, as a share of the
/// foreground. The chip paints with it and `Theme::chrome_ink` floors against
/// it.
pub(crate) const PORT_CHIP_WASH: f32 = 0.06;

impl Global for ChromeInk {}

/// The colour every step-toward-the-ink chrome fill moves toward, cached per
/// theme by `apply_theme` (see `Theme::resolve`). The painters of the fills
/// that are not in `Surfaces` (the settings washes, a port chip) read it.
#[derive(Debug, Clone, Copy)]
pub struct FillPole(pub u32);

impl Global for FillPole {}

/// One theme change's worth of chrome derivation: the resolved pole, the
/// neutrals and surfaces built from it, and the sets of fills each ink role
/// is painted on, so the pole criterion, the cached inks and the sweeps all
/// read the same fills.
pub(crate) struct Resolved {
    pub pole: u32,
    pub m: Neutrals,
    pub sf: Surfaces,
    raise: f32,
    menu_lift: f32,
    caption: Vec<u32>,
    resting: Vec<u32>,
    every_rung: Vec<u32>,
    settings_text: Vec<u32>,
    settings_marks: Vec<u32>,
    settings_glyph: Vec<u32>,
    settings_status: Vec<u32>,
    settings_warn: Vec<u32>,
    panel: Vec<u32>,
    popover_rows: Vec<u32>,
    port_chip: Vec<u32>,
    selected: Vec<u32>,
}

impl Resolved {
    /// Each set of fills with the floor its ink must clear on all of them.
    fn lists(&self) -> [(&[u32], f32); 12] {
        [
            (&self.caption, TEXT_FLOOR),
            (&self.resting, TEXT_FLOOR),
            (&self.resting, ACCENT_FLOOR),
            (&self.every_rung, ACCENT_FLOOR),
            (&self.settings_text, TEXT_FLOOR),
            (&self.settings_marks, ACCENT_FLOOR),
            (&self.settings_glyph, ACCENT_FLOOR),
            (&self.settings_status, TEXT_FLOOR),
            (&self.panel, TEXT_FLOOR),
            (&self.popover_rows, TEXT_FLOOR),
            (&self.port_chip, TEXT_FLOOR),
            (&self.selected, TEXT_FLOOR),
        ]
    }

    /// Whether white or black clears every set's floor on every fill in it.
    fn shares_one_ink(&self) -> bool {
        self.lists().iter().all(|&(fills, floor)| {
            [0xffffff, 0x000000]
                .iter()
                .any(|&ink| fills.iter().all(|&fill| contrast(ink, fill) >= floor))
        })
    }
}

/// How far each settings wash steps off what it sits on, as `(light, dark)`
/// shares of the ink, and how far a raised control lifts off the page. The
/// page paints with these and `Theme::settings_ink` floors against them.
pub(crate) mod settings_wash {
    /// A hovered or selected row.
    pub const ROW: (f32, f32) = (0.04, 0.055);
    /// A keycap, a segmented track, a pressed control.
    pub const KEYCAP: (f32, f32) = (0.05, 0.07);
    /// A highlighted menu row.
    pub const MENU_ROW: (f32, f32) = (0.06, 0.08);
    /// Lightness a light control sits above the page.
    pub const RAISE: f32 = 0.035;
    /// Lightness a dark popover sits above the page.
    pub const MENU_LIFT_DARK: f32 = 0.06;
}

/// A wash of `pole` at `share` laid over `surface`, composited and rounded the
/// way the painters' `Hsla` washes are (blend, then eight bits), so the fills
/// the floors are measured on are the fills that are drawn, to the channel.
fn wash_over(surface: u32, pole: u32, share: f32) -> u32 {
    let wash = Hsla::from(gpui::rgb(pole)).opacity(share);
    pack(Hsla::from(gpui::rgb(surface)).blend(wash))
}

/// `c` moved `by` in lightness, in the colour's own hue.
pub(crate) fn lift(c: Hsla, by: f32) -> Hsla {
    Hsla {
        l: (c.l + by).clamp(0., 1.),
        ..c
    }
}

/// UI interaction roles. Terminal ANSI colours are deliberately independent.
#[derive(Debug, Clone, Copy)]
pub struct Interactions {
    pub primary: Semantic,
    pub primary_hover: u32,
    pub primary_pressed: u32,
    pub choice: u32,
    pub choice_ink: u32,
    pub input_border: u32,
}

impl Global for Interactions {}

pub mod state {
    pub const HOVER: f32 = 1.18;
    pub const SELECTED: f32 = 1.30;
    /// The least CIE L* a rung must sit past the one before it (base, hover,
    /// selected) to read as a separate state. Measured minima are 1.33 across
    /// the built-ins and 1.02 across the generated grid (the left rail's
    /// hover-to-selected step on a mid-grey base); this sits under both with
    /// room for a rounded channel.
    pub const RUNG_MIN_DELTA: f32 = 0.9;
    pub const PRESSED: f32 = 1.55;
    pub const CURSOR: f32 = 1.70;
    /// The side panels' own, lighter rungs (see `Theme::panel_surface`).
    /// A panel lists many rows at once, and at the window's 1.30 the current
    /// one read as a dark slab; these land on #ededec / #e8e8e7 on the
    /// default light rail (#f5f5f4).
    pub const PANEL_HOVER: f32 = 1.07;
    pub const PANEL_SELECTED: f32 = 1.12;
    pub const TEXT_RESTING: f32 = 4.6;
    pub const TEXT_STEP: f32 = 1.4;
}

#[derive(Debug, Clone, Copy)]
pub struct Surface {
    pub base: u32,
    pub hover: u32,
    pub selected: u32,
    pub pressed: u32,
    /// The top rung. No surface paints it today; the ladder tests hold the
    /// rungs below it apart against it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub cursor: u32,
    pub text_resting: u32,
    pub text_selected: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Scrim {
    pub ink: u32,
    pub alpha: f32,
}

#[derive(Debug, Clone)]
pub struct Surfaces {
    pub window: Surface,
    pub sidebar: Surface,
    /// The left tab rail's ladder, built on `Neutrals::rail`.
    pub rail: Surface,
    pub popover: Surface,
    pub scrim: Scrim,
}

impl Global for Surfaces {}

pub fn scrim_fill(cx: &App) -> Hsla {
    let s = cx.global::<Surfaces>().scrim;
    Hsla::from(gpui::rgb(s.ink)).opacity(s.alpha)
}

pub struct ActiveAccent(pub u32);

impl Global for ActiveAccent {}

/// How many lanes of the commit graph get a colour of their own.
///
/// Six because that is how many hues of the ANSI set survive being pulled to a
/// contrast floor while staying apart from each other — and because the graph
/// caps its visible lanes at the same number, which is what guarantees no two
/// columns on screen are ever the same colour.
pub const LANE_SLOTS: usize = 6;

#[derive(Debug, Clone, Copy)]
pub struct Lanes {
    pub ink: [u32; LANE_SLOTS],
    /// Everything past the last slot shares one column, so it gets a neutral:
    /// a hue there would claim a branch identity the column does not have.
    pub overflow: u32,
}

pub struct ActiveLanes(pub Lanes);

impl Global for ActiveLanes {}

impl Theme {
    pub fn background_color(&self) -> u32 {
        self.background.color()
    }

    pub fn neutrals(&self) -> Neutrals {
        self.resolve().m
    }

    /// The neutral fills with every step-toward-the-ink derivation (the
    /// rail's tint, a dark popover, the rungs' shade) moving toward `pole`.
    fn neutrals_at(&self, pole: u32) -> Neutrals {
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        // One continuous surface: the rails share the window's fill and are
        // told apart from the pane area by a hairline alone. A tinted rail
        // made the workspace read as three boxes set side by side.
        let sidebar = bg;
        // The left rail is the one tinted surface: 3% toward the ink, which
        // lands on #f5f5f3 / #1e1e20 for the default pair. Everything painted
        // on it is floored against it below, the same as the other fills.
        let rail = mix(bg, pole, RAIL_TINT);
        // Light overlays sit above the grey rail; dark overlays lift toward
        // the foreground. A light menu must not be darker than its backdrop.
        let popover = if self.dark { mix(bg, pole, 0.06) } else { bg };
        // Two weights, one derivation. Which one a line gets is decided by
        // whether it is the *only* thing separating what it sits between:
        //
        // - `border` closes an outline around a surface that floats on top of
        //   other content (menu, tooltip, dialog, card) and rules a header off
        //   from the rows under it. Nothing else says where the edge is, so it
        //   has to be visible.
        // - `divider` runs between two panes that already carry their own
        //   fills — the sidebar against the terminal, the right panel against
        //   the workspace. There the line is the second signal, not the first,
        //   and painting it at full weight is what makes a workspace read as
        //   boxes bolted together rather than one surface.
        //
        // Both are floored on all three neutral fills, not only on the window:
        // the same value has to be worth something wherever it is painted.
        let hairline = |seed: f32, floor: f32| {
            [bg, sidebar, rail, popover]
                .into_iter()
                .fold(mix(bg, fg, seed), |ink, surface| {
                    at_least(ink, fg, surface, floor)
                })
        };
        let border = hairline(0.16, BORDER_FLOOR);
        let divider = hairline(0.16, DIVIDER_FLOOR);
        let (rail_hover, rail_selected) = panel_rungs(rail, pole);
        // This shared caption role is painted on all three opaque surfaces plus
        // the left rail's hover and selected fills. Calibrating only against
        // resting surfaces leaves light-mode captions under 4.5:1 on hover,
        // selected rows, and the rail's search well.
        let muted_foreground = legible_across(
            &[bg, sidebar, rail, popover, rail_hover, rail_selected],
            dim(fg, bg, state::TEXT_RESTING),
            TEXT_FLOOR,
        );
        let title_floor = TITLE_FLOOR.max(contrast(muted_foreground, sidebar) * state::TEXT_STEP);
        let rail_title_floor = TITLE_FLOOR.max(contrast(muted_foreground, rail) * state::TEXT_STEP);
        // Blended, not bisected, so a palette's own softness carries into the
        // rails — but floored on the fill it is actually painted on. This is
        // the tab title, the top rung of a three-rung column (title / branch /
        // group header), and it is floored at `TITLE_FLOOR` rather than
        // `TEXT_FLOOR` because the rung under it, `muted_foreground`, already
        // sits at `TEXT_RESTING`: a title at 4.5:1 next to a caption at 4.6:1
        // is the same grey twice, and the column reads as one flat wash with
        // nothing to look at first.
        let title_ink = |surface: u32, floor: f32| {
            let title = legible_ink(surface, mix(fg, bg, 0.10), floor);
            // …and capped so the selected label keeps its `TEXT_STEP` above
            // it: on a white-on-black palette a 10% blend lands so close to
            // `fg` that there is nothing brighter left to step to. The floor
            // wins over the cap on a soft palette, where the step is taken
            // past `fg` instead (see `stepped_ink`).
            let headroom = (contrast(fg, surface) / state::TEXT_STEP).max(floor);
            match headroom > floor && contrast(title, surface) > headroom {
                true => dim(title, surface, headroom),
                false => title,
            }
        };
        Neutrals {
            background: bg,
            foreground: fg,
            border,
            divider,
            secondary: mix(bg, pole, 0.09),
            muted: mix(bg, pole, 0.06),
            muted_foreground,
            popover,
            caret: legible_ink(bg, self.caret.unwrap_or(self.accent), ACCENT_FLOOR),
            selection: self.selection.unwrap_or_else(|| mix(bg, pole, 0.20)),
            sidebar,
            // Blended, not bisected, so a palette's own softness carries into
            // the sidebar — but floored on the fill it is actually painted on
            // (`sidebar`, not `background`). This is the tab title, the top
            // rung of a three-rung column (title / branch / group header), and
            // it is floored at `TITLE_FLOOR` rather than `TEXT_FLOOR` because
            // the rung under it, `muted_foreground`, already sits at
            // `TEXT_RESTING`: a title at 4.5:1 next to a caption at 4.6:1 is
            // the same grey twice, and the column reads as one flat wash with
            // nothing to look at first.
            sidebar_fg: title_ink(sidebar, title_floor),
            rail,
            rail_fg: title_ink(rail, rail_title_floor),
            accent: legible_accent(bg, self.accent),
        }
    }

    fn surface_at(&self, base: u32, pole: u32) -> Surface {
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        let hover = raise(base, pole, state::HOVER);
        let selected = legible_rung(raise(base, pole, state::SELECTED), hover, base, fg);
        let text_resting = dim(fg, base, state::TEXT_RESTING);
        // A palette too weak to hold its own text on a selection that still
        // reads as a step keeps the step; the title takes the lift instead.
        let text_selected = match contrast(fg, selected) < TEXT_FLOOR {
            true => lift_lightness(fg, selected, text_resting),
            false => stepped_ink(selected, base, fg, text_resting),
        };
        Surface {
            base,
            hover,
            selected,
            pressed: raise(base, pole, state::PRESSED),
            cursor: raise(base, pole, state::CURSOR),
            text_resting,
            text_selected,
        }
    }

    /// A side panel's surface — the tab rail on the left, the docked panel on
    /// the right. Both list many rows at once, so they step on the lighter
    /// `PANEL_*` rungs, and toward the panel's own shade rather than `fg`: a
    /// cool foreground (the default light's #1c1c1e) bleaches a warm panel's
    /// tint out of a rung this faint, and the row reads as a grey patch on
    /// cream.
    fn panel_surface(&self, base: u32, text_resting: u32, fg: u32, pole: u32) -> Surface {
        let (hover, selected) = panel_rungs(base, pole);
        Surface {
            hover,
            selected,
            text_resting,
            text_selected: stepped_ink(selected, base, fg, text_resting),
            ..self.surface_at(base, pole)
        }
    }

    #[cfg(test)]
    pub(crate) fn interactions(&self) -> Interactions {
        self.interactions_with(&self.neutrals())
    }

    pub(crate) fn interactions_with(&self, m: &Neutrals) -> Interactions {
        // Menus use a blue wash; the switcher uses the solid companion fill.
        // Keep labels and shortcut hints readable on the shared menu surface.
        let choice = mix(m.popover, 0x2878df, if self.dark { 0.34 } else { 0.20 });
        let preferred_label = if self.dark { 0x121418 } else { 0xffffff };
        let fill = self.clear_ink(
            m,
            legible_ink(preferred_label, m.accent, TEXT_FLOOR),
            ACCENT_FLOOR,
        );
        // A filled primary action keeps the same readable label while its
        // hover/pressed background moves away from that label.
        let on_fill = ink_on(fill, preferred_label, TEXT_FLOOR);
        let away = if is_dark(on_fill) { 0xffffff } else { 0x000000 };
        Interactions {
            primary: Semantic {
                ink: self.clear_ink(m, m.accent, TEXT_FLOOR),
                fill,
                on_fill,
            },
            primary_hover: mix(fill, away, 0.08),
            primary_pressed: mix(fill, away, 0.16),
            choice,
            choice_ink: legible_foreground(choice, m.foreground),
            input_border: m.border,
        }
    }

    fn ansi_seed(&self, index: usize) -> u32 {
        let (r, g, b) = self.ansi16[index];
        (r as u32) << 16 | (g as u32) << 8 | b as u32
    }

    /// Clears a seed colour for use as ink on any of tty59's neutral fills.
    ///
    /// An error line lands on a popover or a sidebar row as often as on the
    /// window, and both of those fills sit a step toward the foreground. Clear
    /// the floor on every surface the ink can be painted on, not just on the
    /// darkest one.
    fn clear_ink(&self, m: &Neutrals, seed: u32, floor: f32) -> u32 {
        legible_across(&[m.background, m.sidebar, m.rail, m.popover], seed, floor)
    }

    /// One entry of the theme's own ANSI ramp, legible as chrome text.
    ///
    /// The ramp is authored for terminal cells, where a colour only has to
    /// stand on the background. Chrome draws the same hues on the sidebar and
    /// popover fills too, so they go through the same floor the semantic inks
    /// do.
    #[cfg(test)]
    pub fn ansi_ink(&self, index: usize) -> u32 {
        self.ansi_ink_with(&self.neutrals(), index)
    }

    pub(crate) fn ansi_ink_with(&self, m: &Neutrals, index: usize) -> u32 {
        self.clear_ink(m, self.ansi_seed(index), TEXT_FLOOR)
    }

    #[cfg(test)]
    pub fn semantics(&self) -> Semantics {
        self.semantics_with(&self.neutrals())
    }

    pub(crate) fn semantics_with(&self, m: &Neutrals) -> Semantics {
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        let build = |seed: u32| {
            let fill = self.clear_ink(m, seed, ACCENT_FLOOR);
            Semantic {
                ink: self.clear_ink(m, seed, TEXT_FLOOR),
                fill,
                on_fill: ink_on(fill, fg, TEXT_FLOOR),
            }
        };
        // Green and amber are the v4 Git inks (added / modified); the floors
        // in `build` still walk them to legibility on every surface.
        let (red, green, amber) = if self.dark {
            (0xf07878, 0x4cc27a, 0xe0a345)
        } else {
            (0xc43c43, 0x2f8a52, 0xb7791f)
        };
        Semantics {
            danger: build(red),
            success: build(green),
            warning: build(amber),
            info: build(self.accent),
            link: build(self.accent),
        }
    }

    /// The one colour every step-toward-the-ink chrome fill (the rail's tint,
    /// a dark popover, the state rungs, the settings washes, a port chip's
    /// wash) moves toward, with everything derived from it. Resolved once per
    /// theme change, in `apply_theme`; nothing else chooses a pole.
    ///
    /// The authored foreground is the pole, unless the fills it produces
    /// cannot all be served by one ink at their floors (4.5:1 for text, 3:1
    /// for marks): the palette sits where white and black serve equally, and
    /// stepping toward the ink walks its fills out from under it. Then the
    /// fills step the other way, away from the ink, which raises every ink's
    /// contrast. The authored background, foreground and every terminal colour
    /// come out unchanged; only the direction of the chrome's own fills moves.
    /// Not steps toward the ink, and so not on the pole: borders, dividers and
    /// rings (strokes floored on every chrome fill), the scrollbar thumb, the
    /// switch knob, the scrim and the blue menu choice wash.
    pub(crate) fn resolve(&self) -> Resolved {
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        let authored = self.derive(fg);
        if authored.shares_one_ink() {
            return authored;
        }
        let away = match relative_luminance(fg) > relative_luminance(bg) {
            true => 0x000000,
            false => 0xffffff,
        };
        self.derive(away)
    }

    /// Every chrome fill for `pole`, and the sets of fills each ink role is
    /// painted on. The one place those sets are written down: the pole
    /// criterion, the derived inks and the sweeps all read them.
    fn derive(&self, pole: u32) -> Resolved {
        let m = self.neutrals_at(pole);
        let sf = self.surfaces_at(&m, pole);
        let page = m.background;
        let share = |(light, dark): (f32, f32)| if self.dark { dark } else { light };
        let wash = |surface: u32, strength: (f32, f32)| wash_over(surface, pole, share(strength));
        // Lifts follow the pole too: a dark surface lifts toward it, a light
        // one away from it, which is what they already do toward the ink.
        let toward = if relative_luminance(pole) >= relative_luminance(page) {
            1.
        } else {
            -1.
        };
        let sign = if self.dark { toward } else { -toward };
        let raise = settings_wash::RAISE * sign;
        let menu_lift = settings_wash::MENU_LIFT_DARK * sign;
        let menu = pack(lift(
            gpui::rgb(page).into(),
            if self.dark { menu_lift } else { raise },
        ));
        Resolved {
            pole,
            raise,
            menu_lift,
            caption: vec![
                m.background,
                m.sidebar,
                m.rail,
                m.popover,
                sf.rail.hover,
                sf.rail.selected,
            ],
            resting: vec![m.background, m.sidebar, m.rail, m.popover],
            every_rung: vec![
                sf.rail.base,
                sf.rail.hover,
                sf.rail.selected,
                sf.window.base,
                sf.window.hover,
                sf.window.selected,
            ],
            settings_text: vec![
                page,
                wash(page, settings_wash::ROW),
                wash(page, settings_wash::KEYCAP),
                menu,
                wash(menu, settings_wash::MENU_ROW),
                sf.rail.base,
                sf.rail.hover,
                sf.rail.selected,
                wash(sf.rail.base, settings_wash::KEYCAP),
            ],
            settings_marks: vec![
                page,
                wash(page, settings_wash::ROW),
                wash(page, settings_wash::KEYCAP),
                sf.rail.base,
                wash(sf.rail.base, settings_wash::KEYCAP),
            ],
            // The search glass: on the page, in the nav rail's search well,
            // and on the open dropdown's own fill.
            settings_glyph: vec![
                page,
                wash(page, settings_wash::ROW),
                sf.rail.base,
                wash(sf.rail.base, settings_wash::ROW),
                menu,
            ],
            // Error, Unsaved and Live text: on the page, on a hovered or open
            // host row and the shortcut conflict card, in a dropdown and on
            // its highlighted row.
            settings_status: vec![
                page,
                wash(page, settings_wash::ROW),
                menu,
                wash(menu, settings_wash::MENU_ROW),
            ],
            settings_warn: vec![page, wash(page, settings_wash::ROW)],
            panel: vec![
                sf.sidebar.base,
                sf.sidebar.hover,
                sf.sidebar.selected,
                sf.rail.base,
                sf.rail.hover,
                sf.rail.selected,
            ],
            popover_rows: vec![sf.popover.base, sf.popover.hover, sf.popover.selected],
            port_chip: [
                sf.sidebar.base,
                sf.sidebar.hover,
                sf.window.base,
                sf.window.hover,
            ]
            .map(|row| wash_over(row, pole, PORT_CHIP_WASH))
            .to_vec(),
            selected: vec![
                sf.window.selected,
                sf.popover.selected,
                sf.sidebar.selected,
                sf.rail.selected,
            ],
            m,
            sf,
        }
    }

    /// The settings page's secondary ink and off-switch track, floored on the
    /// fills they are painted on. Derived once per theme change, in
    /// `apply_theme`, so a page that draws hundreds of rows reads cached
    /// colours instead of bisecting contrast per frame.
    #[cfg(test)]
    pub fn settings_ink(&self) -> SettingsInk {
        self.settings_ink_from(&self.resolve())
    }

    pub(crate) fn settings_ink_from(&self, r: &Resolved) -> SettingsInk {
        let sem = self.semantics_with(&r.m);
        let (sub_share, off_share) = if self.dark {
            (0.55, 0.40)
        } else {
            (0.65, 0.50)
        };
        SettingsInk {
            sub: legible_across(
                &r.settings_text,
                mix(r.m.background, r.m.foreground, sub_share),
                TEXT_FLOOR,
            ),
            switch_off: legible_across(
                &r.settings_marks,
                mix(r.m.background, r.m.foreground, off_share),
                ACCENT_FLOOR,
            ),
            glyph: legible_across(
                &r.settings_glyph,
                mix(r.m.background, r.m.foreground, 0.5),
                ACCENT_FLOOR,
            ),
            danger: legible_across(&r.settings_status, sem.danger.ink, TEXT_FLOOR),
            warn_text: legible_across(&r.settings_status, sem.warning.ink, TEXT_FLOOR),
            ok: legible_across(&r.settings_status, sem.success.ink, TEXT_FLOOR),
            warn: legible_across(&r.settings_warn, sem.warning.fill, ACCENT_FLOOR),
            raise: r.raise,
            menu_lift: r.menu_lift,
        }
    }

    /// Ink for the switcher, search overlay and pane grips, floored on the
    /// popover and window rungs they are painted on (`Surfaces`, the same
    /// fills the views read). Derived once per theme change, in `apply_theme`.
    #[cfg(test)]
    pub fn chrome_ink(&self) -> ChromeInk {
        self.chrome_ink_from(&self.resolve())
    }

    pub(crate) fn chrome_ink_from(&self, r: &Resolved) -> ChromeInk {
        let (m, sf) = (&r.m, &r.sf);
        let fg = m.foreground;
        let keycap_fill = mix(sf.popover.base, r.pole, 0.06);
        // A row that is not the picked one is only ever resting or hovered.
        let unpicked = [sf.popover.base, sf.popover.hover];
        ChromeInk {
            selected_meta: legible_across(
                &[sf.popover.selected],
                mix(sf.popover.selected, fg, 0.72),
                TEXT_FLOOR,
            ),
            quiet_name: legible_across(&r.popover_rows, mix(sf.popover.base, fg, 0.65), TEXT_FLOOR),
            avatar_ink: legible_across(&[m.secondary], mix(m.secondary, fg, 0.80), TEXT_FLOOR),
            keycap_fill,
            keycap_ink: legible_across(&[keycap_fill], mix(keycap_fill, fg, 0.70), TEXT_FLOOR),
            grip_idle: legible_across(
                &[sf.window.base],
                mix(sf.window.base, fg, 0.50),
                ACCENT_FLOOR,
            ),
            search_title: legible_across(
                &r.popover_rows,
                mix(sf.popover.base, fg, 0.92),
                TEXT_FLOOR,
            ),
            panel_title: legible_across(&r.panel, fg, TEXT_FLOOR),
            port_chip_ink: legible_across(&r.port_chip, fg, TEXT_FLOOR),
            port_chip_hover_ink: legible_across(
                &r.port_chip,
                self.semantics_with(m).link.ink,
                TEXT_FLOOR,
            ),
            unpicked_title: legible_across(&unpicked, fg, TEXT_FLOOR),
            unpicked_meta: legible_across(&unpicked, m.muted_foreground, TEXT_FLOOR),
        }
    }

    /// Lane colours for the commit graph, derived the same way every other
    /// colour in this file is: seeded from the theme's own palette, then walked
    /// to a contrast floor on each surface it can be painted on.
    ///
    /// Not a fixed table of hexes. A hard-coded palette would be the one thing
    /// here that does not follow the theme, and — worse — the contrast tests
    /// below cannot see it, so the four light builtins would ship a graph whose
    /// lanes sit at 2:1 against their own background.
    ///
    /// The seed order is blue, yellow, magenta, green, cyan, red. Three
    /// constraints picked it: no two adjacent slots share a hue family; red and
    /// green are never neighbours, for the readers who cannot tell them apart;
    /// and red is last because a panel three or four lanes wide never reaches
    /// it, so the one colour that also means "danger" everywhere else in the UI
    /// stays out of the common case.
    #[cfg(test)]
    pub fn lanes(&self) -> Lanes {
        self.lanes_with(&self.neutrals())
    }

    pub(crate) fn lanes_with(&self, m: &Neutrals) -> Lanes {
        const SEEDS: [usize; LANE_SLOTS] = [4, 3, 5, 2, 6, 1];
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        // Through `clear_ink`, the same three surfaces `semantics` clears on:
        // the graph draws on the window in a floating panel, on the sidebar
        // when the panel is docked, and on a popover in the detail view.
        let mut ink = [0u32; LANE_SLOTS];
        for (slot, seed) in SEEDS.iter().enumerate() {
            ink[slot] = self.clear_ink(m, self.ansi_seed(*seed), ACCENT_FLOOR);
        }
        Lanes {
            ink,
            overflow: dim(fg, bg, state::TEXT_RESTING),
        }
    }

    #[cfg(test)]
    pub fn surfaces(&self) -> Surfaces {
        self.resolve().sf
    }

    fn surfaces_at(&self, m: &Neutrals, pole: u32) -> Surfaces {
        let fg = legible_foreground(self.background_color(), self.foreground);
        // The panels once climbed a rung *above* the window (1.50), when a
        // regular-weight current tab was the faintest mark in its column. The
        // current title is SEMIBOLD now and carries that on its own; a heavy
        // fill on top of it read as a pressed button rather than as a place.
        let sidebar = self.panel_surface(m.sidebar, m.sidebar_fg, fg, pole);
        let rail = self.panel_surface(m.rail, m.rail_fg, fg, pole);
        Surfaces {
            window: self.surface_at(m.background, pole),
            sidebar,
            rail,
            popover: self.surface_at(m.popover, pole),
            // The v5 scrim: a dim, not a blackout. Dark lands on near-black at
            // 45%; light on a warm near-black at 14%, just enough to say the
            // window is waiting without turning the page grey.
            scrim: Scrim {
                ink: mix(m.background, 0x000000, 0.92),
                alpha: match self.dark {
                    true => 0.45,
                    false => 0.14,
                },
            },
        }
    }

    pub fn active_palette(&self, legible: bool) -> ActivePalette {
        let bg = self.background_color();
        let fg = legible_foreground(bg, self.foreground);
        let mut ansi16 = [Rgb { r: 0, g: 0, b: 0 }; 16];
        for (i, (r, g, b)) in self.ansi16.iter().enumerate() {
            let ink = (*r as u32) << 16 | (*g as u32) << 8 | *b as u32;
            // The bright half of the palette (SGR 90-97) is the shell's
            // readable-variant family: PSReadLine paints parameters, operators
            // and members with it, so a bright slot that cannot clear the text
            // floor on the theme background is walked toward the foreground
            // until it can — the same rescue `sidebar_fg` uses. Dark themes
            // tend to author it as a muted grey (invisible), light themes as a
            // pale grey (invisible the other way); both are lifted here, while
            // already-legible slots render byte-for-byte as authored. The
            // `legible` flag mirrors Settings → Appearance: off renders the
            // palette exactly as authored.
            ansi16[i] = rgb_bytes(if legible && i >= 8 {
                at_least(ink, fg, bg, TEXT_FLOOR)
            } else {
                ink
            });
        }
        ActivePalette {
            ansi16,
            sel_bg: rgb_bytes(mix(bg, fg, 0.24)),
        }
    }

    fn from_builtin(b: &BuiltinSpec) -> Theme {
        let bg = b.background;
        Theme {
            id: b.id.to_string(),
            name: b.name.to_string(),
            dark: is_dark(bg),
            background: Fill::Solid(bg),
            foreground: b.foreground,
            accent: b.accent,
            caret: b.caret,
            selection: None,
            opacity: None,
            blur: false,
            image: None,
            ansi16: b.ansi16,
            path: None,
        }
    }
}

fn bisect_contrast(from: u32, toward: u32, against: u32, target: f32) -> u32 {
    const STEPS: u32 = 12;
    let rising = contrast(toward, against) > contrast(from, against);
    if rising && contrast(toward, against) <= target {
        return toward;
    }
    if !rising && contrast(toward, against) >= target {
        return toward;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..STEPS {
        let m = 0.5 * (lo + hi);
        let reached = if rising {
            contrast(mix(from, toward, m), against) >= target
        } else {
            contrast(mix(from, toward, m), against) <= target
        };
        if reached { hi = m } else { lo = m }
    }
    mix(from, toward, hi)
}

fn panel_rungs(base: u32, pole: u32) -> (u32, u32) {
    let shade = shade_of(base, pole);
    (
        raise(base, shade, state::PANEL_HOVER),
        raise(base, shade, state::PANEL_SELECTED),
    )
}

fn raise(base: u32, toward: u32, target: f32) -> u32 {
    bisect_contrast(base, toward, base, target)
}

/// A selected rung the surface's own text can still be read on. The full step
/// off the base leaves a weak foreground under the text floor on some palettes
/// (One Dark Pro's popover lands on 4.46:1), so the step is shortened to the
/// last fill that holds the floor, as long as the selection still sits
/// `state::RUNG_MIN_DELTA` past hover, on hover's far side from the base.
/// Where the floor and the order cannot both hold, the order wins and
/// `lift_lightness` carries the title.
fn legible_rung(selected: u32, hover: u32, base: u32, fg: u32) -> u32 {
    if contrast(fg, selected) >= TEXT_FLOOR || contrast(fg, base) < TEXT_FLOOR {
        return selected;
    }
    let shortened = bisect_contrast(selected, base, fg, TEXT_FLOOR);
    let past_hover = |rung: u32| {
        (lightness(rung) - lightness(hover)) * (lightness(selected) - lightness(hover)).signum()
    };
    match past_hover(shortened) >= state::RUNG_MIN_DELTA {
        true => shortened,
        false => selected,
    }
}

/// CIE L* of a packed sRGB colour: the perceptual lightness `RUNG_MIN_DELTA`
/// is measured in.
fn lightness(c: u32) -> f32 {
    let y = relative_luminance(c);
    let f = if y > 216. / 24389. {
        y.cbrt()
    } else {
        (24389. / 27. * y + 16.) / 116.
    };
    116. * f - 16.
}

/// `ink` moved in HSL lightness alone, hue and saturation kept, until it holds
/// the resting-text floor on `fill` and `TEXT_STEP` over the quieter `resting`
/// ink. It rises away from the fill first; an ink with no room that way (black
/// on a dark fill) crosses to the other side instead. Starts from the authored
/// ink, so one that already passes is returned as it is.
fn lift_lightness(ink: u32, fill: u32, resting: u32) -> u32 {
    const STEPS: u32 = 64;
    let hsla: Hsla = gpui::rgb(ink).into();
    let at = |l: f32| pack(Hsla { l, ..hsla });
    let holds = |l: f32| {
        contrast(at(l), fill) >= state::TEXT_RESTING && contrast(at(l), resting) >= state::TEXT_STEP
    };
    if holds(hsla.l) {
        return ink;
    }
    let away = if relative_luminance(ink) >= relative_luminance(fill) {
        1.
    } else {
        0.
    };
    [away, 1. - away]
        .into_iter()
        .find_map(|end| {
            (1..=STEPS)
                .map(|step| hsla.l + (end - hsla.l) * step as f32 / STEPS as f32)
                .find(|&l| holds(l))
        })
        .map_or_else(|| at(away), at)
}

/// The darkest (or, on a dark surface, lightest) colour that keeps every
/// channel of `base` the same distance apart, so blending toward it only
/// changes lightness: #f5f5f3 steps to #e8e8e6, never to #e8e8e8.
fn shade_of(base: u32, pole: u32) -> u32 {
    let ch = [base >> 16 & 0xff, base >> 8 & 0xff, base & 0xff];
    let shift = match relative_luminance(pole) < relative_luminance(base) {
        true => ch.map(|c| c - ch.iter().min().unwrap()),
        false => ch.map(|c| c + (0xff - ch.iter().max().unwrap())),
    };
    (shift[0] << 16) | (shift[1] << 8) | shift[2]
}

fn dim(ink: u32, surface: u32, target: f32) -> u32 {
    bisect_contrast(ink, surface, surface, target)
}

fn stepped_ink(fill: u32, base: u32, fg: u32, resting: u32) -> u32 {
    let ink = ink_on(fill, fg, state::TEXT_RESTING);
    if contrast(ink, resting) >= state::TEXT_STEP {
        return ink;
    }
    let away = match relative_luminance(base) < relative_luminance(resting) {
        true => 0xffffff,
        false => 0x000000,
    };
    bisect_contrast(ink, away, resting, state::TEXT_STEP)
}

fn ink_on(fill: u32, fg: u32, target: f32) -> u32 {
    if contrast(fg, fill) >= target {
        return fg;
    }
    let near = if relative_luminance(fg) < relative_luminance(fill) {
        0x000000
    } else {
        0xffffff
    };
    let deepened = bisect_contrast(fg, near, fill, correction_target(target));
    if contrast(deepened, fill) >= target {
        return deepened;
    }
    if contrast(fill, 0xffffff) >= contrast(fill, 0x000000) {
        0xffffff
    } else {
        0x000000
    }
}

/// The colour a glyph is redrawn in once an opaque block caret sits under it.
/// The terminal's own background is the conventional choice and wins in every
/// shipped theme; the foreground is the fallback for a caret close enough to
/// the background to swallow it.
pub(crate) fn caret_ink(caret: Hsla, background: Hsla, foreground: Hsla) -> Hsla {
    let pack = |c: Hsla| {
        let rgb = crate::terminal::palette::hsla_to_rgb(c);
        (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
    };
    let (caret, bg, fg) = (pack(caret), pack(background), pack(foreground));
    if contrast(caret, bg) >= contrast(caret, fg) {
        background
    } else {
        foreground
    }
}

/// Whether a filled shape needs a hairline to stay a shape. A brand colour is a
/// fixed value; a theme background is not, and pure black on a dark window is
/// no shape at all.
pub(crate) fn needs_edge(fill: u32, surface: Hsla) -> bool {
    let rgb = crate::terminal::palette::hsla_to_rgb(surface);
    let packed = (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32;
    contrast(fill, packed) < 1.25
}

/// Whether a surface is dark enough that a halo cut in its own colour stops
/// reading as a ring and starts reading as a hole.
pub(crate) fn surface_is_dark(surface: Hsla) -> bool {
    let rgb = crate::terminal::palette::hsla_to_rgb(surface);
    is_dark((rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32)
}

pub(crate) fn mix(a: u32, b: u32, t: f32) -> u32 {
    let (ar, ag, ab) = (a >> 16 & 0xff, a >> 8 & 0xff, a & 0xff);
    let (br, bg, bb) = (b >> 16 & 0xff, b >> 8 & 0xff, b & 0xff);
    let ch = |x: u32, y: u32| (x as f32 + (y as f32 - x as f32) * t).round() as u32;
    (ch(ar, br) << 16) | (ch(ag, bg) << 8) | ch(ab, bb)
}

fn rgb_bytes(n: u32) -> Rgb {
    Rgb {
        r: (n >> 16) as u8,
        g: (n >> 8) as u8,
        b: n as u8,
    }
}

fn relative_luminance(c: u32) -> f32 {
    fn chan(v: u32) -> f32 {
        let s = v as f32 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * chan(c >> 16 & 0xff) + 0.7152 * chan(c >> 8 & 0xff) + 0.0722 * chan(c & 0xff)
}

#[cfg(test)]
fn channel_distance(a: u32, b: u32) -> u32 {
    let d = |sh: u32| (a >> sh & 0xff).abs_diff(b >> sh & 0xff);
    d(16).max(d(8)).max(d(0))
}

pub(crate) fn contrast(a: u32, b: u32) -> f32 {
    let (l1, l2) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// The opaque ink SGR 2 (faint) text is painted in on a light cell, when the
/// plain fade would drop it under the text floor — `None` when the fade is
/// already legible and should stay a fade.
///
/// Faint text is `opacity` of its ink over the cell. That costs a fixed share
/// of the ink's luminance distance, and on a light background the ratio that
/// distance buys collapses fast: Catppuccin Latte's own foreground fades from
/// 7.1:1 to 3.2:1, and every bright-black the palette rescue lifted to 4.5:1
/// fades back to 2.5:1 — the "illegible secondary text" of #858. So the fade
/// is walked back toward the ink until it clears `TEXT_FLOOR` again, capped at
/// the ink's own ratio: text that was never above the floor is left as dim as
/// it would have been undimmed, not darkened past what the app asked for.
///
/// Light backgrounds only. Dark themes read the same faint text at the same
/// ratios without complaint, and keeping them byte-for-byte as they were is
/// worth more than a symmetric rule nobody asked for. The test is the cell's
/// own background, so a light cell inside a dark theme is rescued too.
pub(crate) fn legible_dim(ink: u32, bg: u32, opacity: f32) -> Option<u32> {
    if is_dark(bg) {
        return None;
    }
    let faded = mix(bg, ink, opacity);
    let floor = TEXT_FLOOR.min(contrast(ink, bg));
    if contrast(faded, bg) >= floor {
        return None;
    }
    Some(bisect_contrast(faded, ink, bg, floor))
}

pub(crate) fn is_dark(bg: u32) -> bool {
    relative_luminance(bg) < 0.5
}

pub(crate) fn is_lighter(a: u32, b: u32) -> bool {
    relative_luminance(a) > relative_luminance(b)
}

const ACCENT_FLOOR: f32 = 3.0;

/// What the glyph painted on top of a match wash keeps for itself.
///
/// WCAG's non-text ratio, not its text one. A match is a state you read
/// *through* for a moment, not a surface you set body copy on — and holding the
/// glyph at 4.5:1 is not a choice this can make anyway: three of the builtins
/// only give their own text 6.6:1, which caps the wash at 1.46:1. That is a
/// hairline's worth of shift (a hairline is 1.5:1) spread over a whole cell,
/// and it is what made a match impossible to find by scanning.
const GLYPH_FLOOR: f32 = 3.0;

/// The two ends of what a wash is allowed to be, whatever the theme.
///
/// The floor buys the weakest palettes a match you can actually see, at the
/// price of a glyph that dips under `GLYPH_FLOOR` on one. The ceiling stops the
/// strongest from painting what reads as a solid block rather than a highlight.
const WASH_FLOOR: f32 = 1.9;
const WASH_CEILING: f32 = 3.2;

/// How far a search match's wash stands off the background it sits on, and how
/// much further the one you are looking at stands off the rest. Returned as
/// `(hit, current)` contrast targets.
///
/// Derived from the theme rather than fixed, because the budget being spent is
/// the theme's: a palette with 21:1 between its text and its background can
/// afford a wash you cannot miss, and one with 6.6:1 cannot. A single constant
/// has to be safe for the second, which leaves the first with nothing.
pub(crate) fn match_wash_targets(bg: u32, fg: u32) -> (f32, f32) {
    let current = (contrast(fg, bg) / GLYPH_FLOOR).clamp(WASH_FLOOR, WASH_CEILING);
    // The plain hits are the crowd the current one has to read out of, so they
    // get a little over half the distance it travels.
    (1.0 + (current - 1.0) * 0.55, current)
}

/// The opacity at which `tint` over `surface` first reaches `target` contrast,
/// as a blended colour.
///
/// A fixed alpha does not survive a theme swap: the terminal's selection tint
/// is `mix(bg, fg, 0.24)`, so painting it at 0.32 is a 7% shift away from the
/// background in *every* theme — invisible on a white one, and barely there on
/// a dark one. Solving for the ratio instead keeps the same wash weight
/// whatever the two colours happen to be.
pub(crate) fn wash(surface: u32, tint: u32, target: f32) -> u32 {
    if contrast(mix(surface, tint, 1.0), surface) < target {
        // The tint cannot get there on its own — a theme whose selection colour
        // is nearly its background. Fall back to the ink that reads on it.
        return legible_ink(surface, tint, target);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        if contrast(mix(surface, tint, mid), surface) >= target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mix(surface, tint, hi)
}

const TEXT_FLOOR: f32 = 4.5;

/// How far the left rail's fill steps off the window toward the ink.
const RAIL_TINT: f32 = 0.03;

/// The floor for the top rung of a text column whose second rung rests at
/// `TEXT_RESTING`. WCAG AAA's 7:1, which also happens to be the smallest
/// ratio that clears `TEXT_STEP` over a 4.6:1 caption with room to spare on
/// the fills a sidebar is actually painted on.
const TITLE_FLOOR: f32 = 7.0;

/// A semantic ink stepped down to sit beside body text instead of over it.
///
/// `success` and `danger` are cleared to `TEXT_FLOOR` at full chroma, which is
/// right for the one line that says a push failed and wrong for a `+94 −26`
/// repeated on every row of a list: twelve saturated numerals become the
/// loudest thing in the column while carrying the least. Blending toward the
/// caption ink they sit next to keeps the hue (green still means added) and
/// takes the shout out, then the blend is walked back toward the full ink
/// only when the surface it lands on cannot carry it at `TEXT_FLOOR`.
pub(crate) fn resting_ink(ink: Hsla, beside: Hsla, surface: Hsla) -> Hsla {
    let (ink, beside, surface) = (pack(ink), pack(beside), pack(surface));
    let blend = mix(ink, beside, 0.45);
    gpui::rgb(at_least(blend, ink, surface, TEXT_FLOOR)).into()
}

fn pack(c: Hsla) -> u32 {
    let rgb = crate::terminal::palette::hsla_to_rgb(c);
    (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
}

/// Hairlines are separators, not control outlines — the surfaces they divide
/// carry their own fills, so WCAG 1.4.11's 3:1 does not apply and painting them
/// that hard would read as a wireframe. This floor only rescues the palettes
/// where the flat blend disappears entirely, so a divider is worth the same
/// amount in every theme.
const BORDER_FLOOR: f32 = 1.5;

/// The same idea one step down, for a line that is not carrying the separation
/// on its own.
///
/// Worth stating because the seed blend cannot say it: `mix(bg, fg, 0.16)`
/// clears neither floor in any builtin theme, so both values are decided
/// entirely here. Lowering the seed changes nothing; this constant is the knob.
const DIVIDER_FLOOR: f32 = 1.2;

/// Keep an authored blend when it already clears `target` on the surface it is
/// painted on, and walk it back toward `toward` only when it does not.
fn at_least(ink: u32, toward: u32, surface: u32, target: f32) -> u32 {
    if contrast(ink, surface) >= target {
        return ink;
    }
    let lifted = bisect_contrast(ink, toward, surface, target);
    if contrast(lifted, surface) >= target {
        return lifted;
    }
    legible_ink(surface, lifted, target)
}

/// Where a cached TEXT correction lands: a hair above the floor it had to
/// reach. A seed that already clears `TEXT_FLOOR` is never touched, but one
/// that does not is walked to 4.6:1 rather than to 4.5 exactly, so
/// antialiasing and 8-bit rounding on thin glyphs cannot take the painted ink
/// back under the floor. Marks (3:1) keep their floor as the target. Only the
/// derive-time helpers (`legible_across`, `ink_on`) use it: `legible_ink` and
/// `at_least` also serve uncached styling and the terminal palette, which keep
/// their exact floors.
const TEXT_CORRECTED: f32 = 4.6;

fn correction_target(floor: f32) -> f32 {
    if floor == TEXT_FLOOR {
        TEXT_CORRECTED
    } else {
        floor
    }
}

fn legible_ink(bg: u32, seed: u32, floor: f32) -> u32 {
    if contrast(seed, bg) >= floor {
        return seed;
    }
    let away = if contrast(0xffffff, bg) >= contrast(0x000000, bg) {
        0xffffff
    } else {
        0x000000
    };
    bisect_contrast(seed, away, bg, floor)
}

/// `seed` kept where it already clears `floor` on every fill, and otherwise
/// walked toward whichever of white and black serves the weakest fill best.
/// `legible_ink` picks its direction fill by fill, which on a mid-grey theme
/// lets two fills pull one ink back and forth; the direction here is fixed
/// once for the whole set, so a floor one ink can hold on all of them is held.
fn legible_across(fills: &[u32], seed: u32, floor: f32) -> u32 {
    let weakest = |ink: u32| {
        fills
            .iter()
            .map(|&fill| contrast(ink, fill))
            .fold(f32::MAX, f32::min)
    };
    let away = if weakest(0xffffff) >= weakest(0x000000) {
        0xffffff
    } else {
        0x000000
    };
    // A seed that clears the floor on every fill is returned as authored. Once any fill
    // misses, the correction aims every fill at the headroom target, so the
    // corrected ink holds it on all of them and not only on the one that failed.
    if fills.iter().all(|&fill| contrast(seed, fill) >= floor) {
        return seed;
    }
    let target = correction_target(floor);
    fills
        .iter()
        .fold(seed, |ink, &fill| match contrast(ink, fill) >= target {
            true => ink,
            false => bisect_contrast(ink, away, fill, target),
        })
}

fn legible_accent(bg: u32, accent: u32) -> u32 {
    legible_ink(bg, accent, ACCENT_FLOOR)
}

fn legible_foreground(bg: u32, fg: u32) -> u32 {
    if contrast(bg, fg) >= 4.5 {
        return fg;
    }
    if contrast(bg, 0xffffff) >= contrast(bg, 0x000000) {
        0xffffff
    } else {
        0x000000
    }
}

pub struct ActiveBackground {
    pub fill: Fill,
    pub opacity: Option<f32>,
    pub image: Option<Image>,
}

impl Global for ActiveBackground {}

pub const DEFAULT_ID: &str = "light";

pub struct Themes(pub Vec<Theme>);

impl Global for Themes {}

/// Files in the themes folder that could not be read, and why. A malformed
/// theme used to log a warning and then simply not be in the list — the folder
/// is one the user opens and drops files into, so "it isn't there" needs a
/// reason attached to it.
#[derive(Default)]
pub struct RejectedThemes(pub Vec<(String, String)>);

impl Global for RejectedThemes {}

pub fn load_registry(cx: &mut App) {
    let (themes, rejected) = load_all();
    cx.set_global(Themes(themes));
    cx.set_global(RejectedThemes(rejected));
}

pub fn all(cx: &App) -> Vec<Theme> {
    cx.try_global::<Themes>()
        .map(|t| t.0.clone())
        .unwrap_or_else(builtins)
}

pub fn rejected(cx: &App) -> Vec<(String, String)> {
    cx.try_global::<RejectedThemes>()
        .map(|r| r.0.clone())
        .unwrap_or_default()
}

pub fn by_id(cx: &App, id: &str) -> Theme {
    let themes = all(cx);
    themes
        .iter()
        .find(|t| t.id == id)
        .or_else(|| themes.iter().find(|t| t.id == DEFAULT_ID))
        .cloned()
        .unwrap_or_else(|| themes.into_iter().next().expect("at least the built-ins"))
}

impl Theme {
    pub fn editable(&self) -> bool {
        self.path
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"))
    }
}

pub fn to_yaml(t: &Theme) -> String {
    fn hex(c: u32) -> String {
        format!("\"#{:06x}\"", c & 0xff_ffff)
    }
    fn rgb_hex((r, g, b): (u8, u8, u8)) -> String {
        format!("\"#{r:02x}{g:02x}{b:02x}\"")
    }
    let mut s = String::new();
    s.push_str(&format!("name: {:?}\n", t.name));
    match &t.background {
        Fill::Solid(c) => s.push_str(&format!("background: {}\n", hex(*c))),
        Fill::Vertical { top, bottom } => s.push_str(&format!(
            "background: {{ top: {}, bottom: {} }}\n",
            hex(*top),
            hex(*bottom)
        )),
        Fill::Horizontal { left, right } => s.push_str(&format!(
            "background: {{ left: {}, right: {} }}\n",
            hex(*left),
            hex(*right)
        )),
    }
    s.push_str(&format!("foreground: {}\n", hex(t.foreground)));
    s.push_str(&format!("accent: {}\n", hex(t.accent)));
    if let Some(c) = t.caret {
        s.push_str(&format!("cursor: {}\n", hex(c)));
    }
    if let Some(c) = t.selection {
        s.push_str(&format!("selection: {}\n", hex(c)));
    }
    if let Some(o) = t.opacity {
        s.push_str(&format!("opacity: {o}\n"));
    }
    if t.blur {
        s.push_str("blur: true\n");
    }
    if let Some(img) = &t.image {
        s.push_str(&format!(
            "background_image:\n  path: {:?}\n  opacity: {}\n",
            img.path.display().to_string(),
            img.opacity
        ));
    }
    let row = |range: std::ops::Range<usize>| {
        range
            .map(|i| rgb_hex(t.ansi16[i]))
            .collect::<Vec<_>>()
            .join(", ")
    };
    s.push_str("ansi:\n");
    s.push_str(&format!("  normal: [{}]\n", row(0..8)));
    s.push_str(&format!("  bright: [{}]\n", row(8..16)));
    s
}

pub fn fork_to_file(t: &Theme) -> std::io::Result<String> {
    let dir = themes_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no themes directory"))?;
    std::fs::create_dir_all(&dir)?;
    let base = format!("{}-custom", t.id.trim_end_matches("-custom"));
    let mut stem = base.clone();
    let mut n = 2;
    while dir.join(format!("{stem}.yaml")).exists() {
        stem = format!("{base}-{n}");
        n += 1;
    }
    let mut copy = t.clone();
    // The name lands in the YAML on disk and is matched back with
    // `trim_end_matches(" (custom)")`, so it stays English in every locale —
    // a translated suffix would survive a language switch and stack up.
    copy.name = format!("{} (custom)", t.name.trim_end_matches(" (custom)"));
    crate::core::config::write_atomic(
        &dir.join(format!("{stem}.yaml")),
        to_yaml(&copy).as_bytes(),
    )?;
    Ok(stem)
}

pub fn write_theme_file(t: &Theme) -> std::io::Result<()> {
    let path = t.path.clone().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "theme is not file-backed")
    })?;
    crate::core::config::write_atomic(&path, to_yaml(t).as_bytes())
}

fn load_all() -> (Vec<Theme>, Vec<(String, String)>) {
    let mut themes = builtins();
    let (user, rejected) = load_user_themes();
    themes.extend(user);
    dedupe_ids(&mut themes);
    (themes, rejected)
}

fn dedupe_ids(themes: &mut [Theme]) {
    let mut seen = std::collections::HashSet::new();
    for t in themes.iter_mut() {
        if seen.insert(t.id.clone()) {
            continue;
        }
        let base = t.id.clone();
        let mut n = 2;
        let mut candidate = format!("{base}-{n}");
        while !seen.insert(candidate.clone()) {
            n += 1;
            candidate = format!("{base}-{n}");
        }
        t.name = format!("{} ({n})", t.name);
        t.id = candidate;
    }
}

pub fn themes_dir() -> Option<PathBuf> {
    crate::core::config::config_path("themes")
}

fn load_user_themes() -> (Vec<Theme>, Vec<(String, String)>) {
    let Some(dir) = themes_dir() else {
        return (Vec::new(), Vec::new());
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (Vec::new(), Vec::new());
    };
    let mut out = Vec::new();
    let mut rejected = Vec::new();
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    for path in paths {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let parsed = match ext.as_deref() {
            Some("yaml") | Some("yml") => load_yaml_theme(&path),
            Some("itermcolors") => load_iterm_theme(&path),
            _ => continue,
        };
        match parsed {
            Ok(theme) => out.push(theme),
            Err(e) => {
                log::warn!("skipping theme {}: {e}", path.display());
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                rejected.push((name, e));
            }
        }
    }
    (out, rejected)
}

fn id_and_name(path: &std::path::Path) -> (String, String) {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("theme")
        .to_string();
    let name = stem
        .split(|c| c == '_' || c == '-' || c == ' ')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    (
        stem,
        if name.is_empty() {
            // Theme names are data — they get written back to the YAML file,
            // so the fallback stays English rather than following the GUI.
            "Theme".into()
        } else {
            name
        },
    )
}

#[derive(Deserialize)]
struct ThemeFile {
    name: Option<String>,
    background: FillFile,
    foreground: String,
    accent: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    selection: Option<String>,
    #[serde(default)]
    opacity: Option<f32>,
    #[serde(default)]
    blur: bool,
    #[serde(default)]
    background_image: Option<ImageFile>,
    ansi: AnsiFile,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FillFile {
    Solid(String),
    Vertical { top: String, bottom: String },
    Horizontal { left: String, right: String },
}

#[derive(Deserialize)]
struct AnsiFile {
    normal: [String; 8],
    bright: [String; 8],
}

#[derive(Deserialize)]
struct ImageFile {
    path: String,
    #[serde(default = "default_image_opacity")]
    opacity: f32,
}

fn default_image_opacity() -> f32 {
    0.3
}

fn load_yaml_theme(path: &std::path::Path) -> Result<Theme, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let file: ThemeFile =
        serde_yaml::from_str(crate::core::config::strip_bom(&text)).map_err(|e| e.to_string())?;
    let (id, derived_name) = id_and_name(path);

    let background = file.background.into_fill()?;
    let bg = background.color();
    let mut ansi16 = [(0u8, 0u8, 0u8); 16];
    for i in 0..8 {
        ansi16[i] = parse_rgb(&file.ansi.normal[i])?;
        ansi16[i + 8] = parse_rgb(&file.ansi.bright[i])?;
    }

    Ok(Theme {
        id,
        name: file.name.unwrap_or(derived_name),
        dark: is_dark(bg),
        background,
        foreground: parse_hex(&file.foreground)?,
        accent: parse_hex(&file.accent)?,
        caret: file.cursor.as_deref().map(parse_hex).transpose()?,
        selection: file.selection.as_deref().map(parse_hex).transpose()?,
        opacity: file.opacity.map(|o| o.clamp(0.0, 1.0)),
        blur: file.blur,
        image: file.background_image.map(|i| Image {
            path: expand_path(&i.path),
            opacity: i.opacity.clamp(0.0, 1.0),
        }),
        ansi16,
        path: Some(path.to_path_buf()),
    })
}

impl FillFile {
    fn into_fill(self) -> Result<Fill, String> {
        Ok(match self {
            FillFile::Solid(s) => Fill::Solid(parse_hex(&s)?),
            FillFile::Vertical { top, bottom } => Fill::Vertical {
                top: parse_hex(&top)?,
                bottom: parse_hex(&bottom)?,
            },
            FillFile::Horizontal { left, right } => Fill::Horizontal {
                left: parse_hex(&left)?,
                right: parse_hex(&right)?,
            },
        })
    }
}

fn expand_path(p: &str) -> PathBuf {
    let p = p.trim();
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    let path = PathBuf::from(p);
    if path.is_absolute() {
        return path;
    }
    themes_dir().map(|d| d.join(&path)).unwrap_or(path)
}

fn load_iterm_theme(path: &std::path::Path) -> Result<Theme, String> {
    let value = plist::Value::from_file(path).map_err(|e| e.to_string())?;
    let dict = value
        .as_dictionary()
        .ok_or("not an iTerm color plist (expected a dictionary)")?;

    let color = |key: &str| -> Option<u32> {
        let c = dict.get(key)?.as_dictionary()?;
        let comp = |k: &str| -> Option<u32> {
            // iTerm2 and plistlib write exact 0 and 1 as `<integer>`.
            let v = c.get(k)?;
            let f = v
                .as_real()
                .or_else(|| v.as_signed_integer().map(|i| i as f64))?;
            Some((f.clamp(0.0, 1.0) * 255.0).round() as u32)
        };
        Some(
            (comp("Red Component")? << 16)
                | (comp("Green Component")? << 8)
                | comp("Blue Component")?,
        )
    };

    let mut ansi16 = [(0u8, 0u8, 0u8); 16];
    for i in 0..16 {
        let c = color(&format!("Ansi {i} Color"))
            .ok_or_else(|| format!("missing or malformed 'Ansi {i} Color'"))?;
        ansi16[i] = ((c >> 16) as u8, (c >> 8) as u8, c as u8);
    }

    let background = color("Background Color").ok_or("missing 'Background Color'")?;
    let foreground = color("Foreground Color").ok_or("missing 'Foreground Color'")?;
    let cursor = color("Cursor Color");
    let bright_blue = {
        let (r, g, b) = ansi16[12];
        (r as u32) << 16 | (g as u32) << 8 | b as u32
    };
    let accent = match cursor {
        Some(c) if contrast(background, c) >= 1.5 => c,
        _ => bright_blue,
    };

    let (id, name) = id_and_name(path);
    Ok(Theme {
        id,
        name,
        dark: is_dark(background),
        background: Fill::Solid(background),
        foreground,
        accent,
        caret: cursor,
        selection: None,
        opacity: None,
        blur: false,
        image: None,
        ansi16,
        path: Some(path.to_path_buf()),
    })
}

fn parse_hex(s: &str) -> Result<u32, String> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return Err(format!("'{s}' is not a 6-digit hex color"));
    }
    u32::from_str_radix(hex, 16).map_err(|_| format!("'{s}' is not a hex color"))
}

fn parse_rgb(s: &str) -> Result<(u8, u8, u8), String> {
    let n = parse_hex(s)?;
    Ok(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

pub fn builtins() -> Vec<Theme> {
    BUILTINS.iter().map(Theme::from_builtin).collect()
}

struct BuiltinSpec {
    id: &'static str,
    name: &'static str,
    background: u32,
    foreground: u32,
    accent: u32,
    caret: Option<u32>,
    ansi16: [(u8, u8, u8); 16],
}

static BUILTINS: [BuiltinSpec; 13] = [
    BuiltinSpec {
        id: "light",
        name: "Light",
        background: 0xfcfcfb,
        foreground: 0x1c1c1e,
        accent: 0x1f6bf0,
        caret: None,
        ansi16: [
            (0x24, 0x29, 0x2e),
            (0xd1, 0x24, 0x2f),
            (0x1a, 0x7f, 0x37),
            (0x9a, 0x67, 0x00),
            (0x09, 0x69, 0xda),
            (0x82, 0x50, 0xdf),
            (0x1b, 0x7c, 0x83),
            (0x6e, 0x77, 0x81),
            (0x57, 0x60, 0x6a),
            (0xcf, 0x22, 0x2e),
            (0x1f, 0x88, 0x3d),
            (0xbf, 0x87, 0x00),
            (0x21, 0x8b, 0xff),
            (0xa4, 0x75, 0xf9),
            (0x31, 0x92, 0xaa),
            (0x8c, 0x95, 0x9f),
        ],
    },
    BuiltinSpec {
        id: "one_light",
        name: "One Light",
        background: 0xfafafa,
        foreground: 0x383a42,
        accent: 0x4078f2,
        caret: None,
        ansi16: [
            (0x38, 0x3a, 0x42),
            (0xe4, 0x56, 0x49),
            (0x50, 0xa1, 0x4f),
            (0xc1, 0x84, 0x01),
            (0x40, 0x78, 0xf2),
            (0xa6, 0x26, 0xa4),
            (0x01, 0x84, 0xbc),
            (0xa0, 0xa1, 0xa7),
            (0x69, 0x6c, 0x77),
            (0xe4, 0x56, 0x49),
            (0x50, 0xa1, 0x4f),
            (0xc1, 0x84, 0x01),
            (0x40, 0x78, 0xf2),
            (0xa6, 0x26, 0xa4),
            (0x01, 0x84, 0xbc),
            (0xfa, 0xfa, 0xfa),
        ],
    },
    BuiltinSpec {
        id: "catppuccin_latte",
        name: "Catppuccin Latte",
        background: 0xeff1f5,
        foreground: 0x4c4f69,
        accent: 0x1e66f5,
        caret: None,
        ansi16: [
            (0xbc, 0xc0, 0xcc),
            (0xd2, 0x0f, 0x39),
            (0x40, 0xa0, 0x2b),
            (0xdf, 0x8e, 0x1d),
            (0x1e, 0x66, 0xf5),
            (0xea, 0x76, 0xcb),
            (0x17, 0x92, 0x99),
            (0x5c, 0x5f, 0x77),
            (0xac, 0xb0, 0xbe),
            (0xd2, 0x0f, 0x39),
            (0x40, 0xa0, 0x2b),
            (0xdf, 0x8e, 0x1d),
            (0x1e, 0x66, 0xf5),
            (0xea, 0x76, 0xcb),
            (0x17, 0x92, 0x99),
            (0x6c, 0x6f, 0x85),
        ],
    },
    BuiltinSpec {
        id: "rose_pine_dawn",
        name: "Rosé Pine Dawn",
        background: 0xfaf4ed,
        foreground: 0x575279,
        accent: 0x907aa9,
        caret: None,
        ansi16: [
            (0xf2, 0xe9, 0xe1),
            (0xb4, 0x63, 0x7a),
            (0x28, 0x69, 0x83),
            (0xea, 0x9d, 0x34),
            (0x56, 0x94, 0x9f),
            (0x90, 0x7a, 0xa9),
            (0xd7, 0x82, 0x7e),
            (0x57, 0x52, 0x79),
            (0x98, 0x93, 0xa5),
            (0xb4, 0x63, 0x7a),
            (0x28, 0x69, 0x83),
            (0xea, 0x9d, 0x34),
            (0x56, 0x94, 0x9f),
            (0x90, 0x7a, 0xa9),
            (0xd7, 0x82, 0x7e),
            (0x57, 0x52, 0x79),
        ],
    },
    BuiltinSpec {
        id: "dark",
        name: "Dark",
        background: 0x18181a,
        foreground: 0xececed,
        accent: 0x78a8f5,
        caret: None,
        ansi16: [
            (0x61, 0x61, 0x61),
            (0xff, 0x82, 0x72),
            (0xb4, 0xfa, 0x72),
            (0xfe, 0xfd, 0xc2),
            (0xa5, 0xd5, 0xfe),
            (0xff, 0x8f, 0xfd),
            (0xd0, 0xd1, 0xfe),
            (0xf1, 0xf1, 0xf1),
            (0x8e, 0x8e, 0x8e),
            (0xff, 0xc4, 0xbd),
            (0xd6, 0xfc, 0xb9),
            (0xfe, 0xfd, 0xd5),
            (0xc1, 0xe3, 0xfe),
            (0xff, 0xb1, 0xfe),
            (0xe5, 0xe6, 0xfe),
            (0xfe, 0xff, 0xff),
        ],
    },
    BuiltinSpec {
        id: "dracula",
        name: "Dracula",
        background: 0x282a36,
        foreground: 0xf8f8f2,
        accent: 0xff79c6,
        caret: None,
        ansi16: [
            (0x00, 0x00, 0x00),
            (0xff, 0x55, 0x55),
            (0x50, 0xfa, 0x7b),
            (0xf1, 0xfa, 0x8c),
            (0xbd, 0x93, 0xf9),
            (0xff, 0x79, 0xc6),
            (0x8b, 0xe9, 0xfd),
            (0xbb, 0xbb, 0xbb),
            (0x55, 0x55, 0x55),
            (0xff, 0x55, 0x55),
            (0x50, 0xfa, 0x7b),
            (0xf1, 0xfa, 0x8c),
            (0xca, 0xa9, 0xfa),
            (0xff, 0x79, 0xc6),
            (0x8b, 0xe9, 0xfd),
            (0xff, 0xff, 0xff),
        ],
    },
    BuiltinSpec {
        id: "harbor",
        name: "Harbor",
        background: 0x1d2022,
        foreground: 0xe4eef5,
        accent: 0x6c96b4,
        caret: None,
        ansi16: [
            (0x12, 0x12, 0x12),
            (0xc7, 0x61, 0x56),
            (0x57, 0xc7, 0x8a),
            (0xc8, 0xa3, 0x5a),
            (0x57, 0x85, 0xc7),
            (0xc7, 0x56, 0xa9),
            (0x57, 0xc7, 0xc3),
            (0xee, 0xed, 0xeb),
            (0x29, 0x29, 0x29),
            (0xd2, 0x2d, 0x1e),
            (0x1c, 0xa0, 0x5a),
            (0xe5, 0xa0, 0x1a),
            (0x14, 0x58, 0xb8),
            (0xa4, 0x37, 0x87),
            (0x4d, 0x99, 0x89),
            (0xff, 0xff, 0xff),
        ],
    },
    BuiltinSpec {
        id: "one_dark_pro",
        name: "One Dark Pro",
        background: 0x282c34,
        foreground: 0xabb2bf,
        accent: 0x528bff,
        caret: None,
        ansi16: [
            (0x3f, 0x44, 0x51),
            (0xe0, 0x6c, 0x75),
            (0x98, 0xc3, 0x79),
            (0xe5, 0xc0, 0x7b),
            (0x61, 0xaf, 0xef),
            (0xc6, 0x78, 0xdd),
            (0x56, 0xb6, 0xc2),
            (0xab, 0xb2, 0xbf),
            (0x5c, 0x63, 0x70),
            (0xff, 0x61, 0x6e),
            (0xa5, 0xe0, 0x75),
            (0xf0, 0xa4, 0x5d),
            (0x4d, 0xc4, 0xff),
            (0xde, 0x73, 0xff),
            (0x4c, 0xd1, 0xe0),
            (0xe6, 0xe6, 0xe6),
        ],
    },
    BuiltinSpec {
        id: "rose_pine",
        name: "Rosé Pine",
        background: 0x191724,
        foreground: 0xe0def4,
        accent: 0xc4a7e7,
        caret: None,
        ansi16: [
            (0x26, 0x23, 0x3a),
            (0xeb, 0x6f, 0x92),
            (0x31, 0x74, 0x8f),
            (0xf6, 0xc1, 0x77),
            (0x9c, 0xcf, 0xd8),
            (0xc4, 0xa7, 0xe7),
            (0xeb, 0xbc, 0xba),
            (0xe0, 0xde, 0xf4),
            (0x6e, 0x6a, 0x86),
            (0xeb, 0x6f, 0x92),
            (0x31, 0x74, 0x8f),
            (0xf6, 0xc1, 0x77),
            (0x9c, 0xcf, 0xd8),
            (0xc4, 0xa7, 0xe7),
            (0xeb, 0xbc, 0xba),
            (0xe0, 0xde, 0xf4),
        ],
    },
    BuiltinSpec {
        id: "catppuccin_mocha",
        name: "Catppuccin Mocha",
        background: 0x1e1e2e,
        foreground: 0xcdd6f4,
        accent: 0x89b4fa,
        // Rosewater, the cursor colour Catppuccin's own terminal spec names —
        // the accent fallback would paint it blue.
        caret: Some(0xf5e0dc),
        ansi16: [
            (0x45, 0x47, 0x5a),
            (0xf3, 0x8b, 0xa8),
            (0xa6, 0xe3, 0xa1),
            (0xf9, 0xe2, 0xaf),
            (0x89, 0xb4, 0xfa),
            (0xf5, 0xc2, 0xe7),
            (0x94, 0xe2, 0xd5),
            (0xba, 0xc2, 0xde),
            (0x58, 0x5b, 0x70),
            (0xf3, 0x8b, 0xa8),
            (0xa6, 0xe3, 0xa1),
            (0xf9, 0xe2, 0xaf),
            (0x89, 0xb4, 0xfa),
            (0xf5, 0xc2, 0xe7),
            (0x94, 0xe2, 0xd5),
            (0xa6, 0xad, 0xc8),
        ],
    },
    BuiltinSpec {
        id: "gruvbox_dark",
        name: "Gruvbox Dark",
        background: 0x282828,
        foreground: 0xebdbb2,
        accent: 0xfe8019,
        caret: Some(0xebdbb2),
        ansi16: [
            (0x28, 0x28, 0x28),
            (0xcc, 0x24, 0x1d),
            (0x98, 0x97, 0x1a),
            (0xd7, 0x99, 0x21),
            (0x45, 0x85, 0x88),
            (0xb1, 0x62, 0x86),
            (0x68, 0x9d, 0x6a),
            (0xa8, 0x99, 0x84),
            (0x92, 0x83, 0x74),
            (0xfb, 0x49, 0x34),
            (0xb8, 0xbb, 0x26),
            (0xfa, 0xbd, 0x2f),
            (0x83, 0xa5, 0x98),
            (0xd3, 0x86, 0x9b),
            (0x8e, 0xc0, 0x7c),
            (0xeb, 0xdb, 0xb2),
        ],
    },
    BuiltinSpec {
        id: "nord",
        name: "Nord",
        background: 0x2e3440,
        foreground: 0xd8dee9,
        accent: 0x88c0d0,
        caret: Some(0xd8dee9),
        ansi16: [
            (0x3b, 0x42, 0x52),
            (0xbf, 0x61, 0x6a),
            (0xa3, 0xbe, 0x8c),
            (0xeb, 0xcb, 0x8b),
            (0x81, 0xa1, 0xc1),
            (0xb4, 0x8e, 0xad),
            (0x88, 0xc0, 0xd0),
            (0xe5, 0xe9, 0xf0),
            (0x4c, 0x56, 0x6a),
            (0xbf, 0x61, 0x6a),
            (0xa3, 0xbe, 0x8c),
            (0xeb, 0xcb, 0x8b),
            (0x81, 0xa1, 0xc1),
            (0xb4, 0x8e, 0xad),
            (0x8f, 0xbc, 0xbb),
            (0xec, 0xef, 0xf4),
        ],
    },
    BuiltinSpec {
        id: "tokyo_night",
        name: "Tokyo Night",
        background: 0x1a1b26,
        foreground: 0xc0caf5,
        accent: 0x7aa2f7,
        caret: Some(0xc0caf5),
        ansi16: [
            (0x15, 0x16, 0x1e),
            (0xf7, 0x76, 0x8e),
            (0x9e, 0xce, 0x6a),
            (0xe0, 0xaf, 0x68),
            (0x7a, 0xa2, 0xf7),
            (0xbb, 0x9a, 0xf7),
            (0x7d, 0xcf, 0xff),
            (0xa9, 0xb1, 0xd6),
            (0x41, 0x48, 0x68),
            (0xf7, 0x76, 0x8e),
            (0x9e, 0xce, 0x6a),
            (0xe0, 0xaf, 0x68),
            (0x7a, 0xa2, 0xf7),
            (0xbb, 0x9a, 0xf7),
            (0x7d, 0xcf, 0xff),
            (0xc0, 0xca, 0xf5),
        ],
    },
];

/// A grid of user-style themes besides the built-ins, for the sweeps that
/// hold a derivation to its floors on palettes nobody tuned: light and dark
/// bases (plain, cream, blue, green and brown tints, and mid greys where no
/// single ink can serve every fill), inks of every strength down to the
/// weakest legible_foreground allows, tinted inks, and three accents.
#[cfg(test)]
pub(crate) fn generated_theme_grid() -> Vec<Theme> {
    const LIGHT_BASES: [u32; 8] = [
        0xffffff, 0xfcfcfb, 0xf5f0e6, 0xe8eef7, 0xdcdcdc, 0xc8d0c4, 0xfff8f0, 0xcccccc,
    ];
    const DARK_BASES: [u32; 10] = [
        0x000000, 0x101014, 0x1e1e2e, 0x2b2b2b, 0x3a2f2f, 0x1a2a1a, 0x303846, 0x404040, 0x555555,
        0x777777,
    ];
    const LIGHT_INKS: [u32; 4] = [0x000000, 0x1a2b4a, 0x4a2a1a, 0x4c4f69];
    const DARK_INKS: [u32; 4] = [0xffffff, 0xd8f0ff, 0xfff0d8, 0xcdd6f4];
    const STRENGTHS: [f32; 4] = [0.55, 0.7, 0.85, 1.0];
    const ACCENTS: [u32; 3] = [0x1f6bf0, 0xe0457b, 0x2fa84f];
    let mut themes = Vec::new();
    for (bases, inks) in [
        (&LIGHT_BASES[..], &LIGHT_INKS[..]),
        (&DARK_BASES[..], &DARK_INKS[..]),
    ] {
        for &background in bases {
            for &ink in inks {
                for strength in STRENGTHS {
                    for accent in ACCENTS {
                        themes.push(Theme {
                            id: format!("grid-{}", themes.len()),
                            name: format!("Grid {background:06x} {ink:06x} {strength}"),
                            dark: is_dark(background),
                            background: Fill::Solid(background),
                            foreground: mix(background, ink, strength),
                            accent,
                            caret: None,
                            selection: None,
                            opacity: None,
                            blur: false,
                            image: None,
                            ansi16: BUILTINS[0].ansi16,
                            path: None,
                        });
                    }
                }
            }
        }
    }
    themes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_tab_row_stays_legible_in_every_builtin_theme() {
        for theme in builtins() {
            let s = theme.surfaces().sidebar;
            assert!(
                contrast(s.selected, s.text_selected) >= 4.5,
                "{} current tab contrast",
                theme.id
            );
            assert_ne!(s.selected, s.base, "{} selection", theme.id);
        }
    }

    #[test]
    fn foreground_is_legible_on_background() {
        for t in builtins() {
            let ratio = contrast(t.background_color(), t.foreground);
            assert!(
                ratio >= 4.0,
                "{}: fg/bg contrast too low ({ratio:.2})",
                t.id
            );
        }
    }

    #[test]
    fn dark_is_inferred_from_background() {
        let dark: Vec<_> = builtins()
            .into_iter()
            .filter(|t| t.dark)
            .map(|t| t.id)
            .collect();
        assert_eq!(
            dark,
            [
                "dark",
                "dracula",
                "harbor",
                "one_dark_pro",
                "rose_pine",
                "catppuccin_mocha",
                "gruvbox_dark",
                "nord",
                "tokyo_night",
            ]
        );
    }

    #[test]
    fn bright_slots_clear_the_text_floor_on_their_background() {
        // The renderer reads this palette for every pane, so every bright slot
        // must be legible on the theme background — SGR 90-97 is the family
        // PSReadLine paints parameters/operators/members with.
        for t in builtins() {
            let bg = t.background_color();
            let ap = t.active_palette(true);
            for (i, c) in ap.ansi16.iter().enumerate().skip(8) {
                let ink = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
                assert!(
                    contrast(ink, bg) >= TEXT_FLOOR - 0.01,
                    "{}/ansi16[{i}]: bright slot {ink:#08x} only {:.2}:1 on the background",
                    t.id,
                    contrast(ink, bg)
                );
            }
        }
    }

    #[test]
    fn legible_bright_slots_keep_their_authored_values() {
        // The lift is a rescue, not a restyle: a bright slot that already
        // clears the floor must come out of `active_palette` untouched.
        for t in builtins() {
            let bg = t.background_color();
            let ap = t.active_palette(true);
            for (i, (r, g, b)) in t.ansi16.iter().enumerate().skip(8) {
                let authored = (*r as u32) << 16 | (*g as u32) << 8 | *b as u32;
                let rendered = (ap.ansi16[i].r as u32) << 16
                    | (ap.ansi16[i].g as u32) << 8
                    | ap.ansi16[i].b as u32;
                if contrast(authored, bg) >= TEXT_FLOOR {
                    assert_eq!(
                        rendered, authored,
                        "{}/ansi16[{i}]: a legible bright slot was touched",
                        t.id
                    );
                }
            }
        }
    }

    #[test]
    fn palette_renders_as_authored_when_legibility_is_off() {
        // The Settings → Appearance switch turns the rescue off entirely: the
        // renderer then sees every slot byte-for-byte as authored.
        for t in builtins() {
            let ap = t.active_palette(false);
            for (i, (r, g, b)) in t.ansi16.iter().enumerate() {
                let rendered = (ap.ansi16[i].r as u32) << 16
                    | (ap.ansi16[i].g as u32) << 8
                    | ap.ansi16[i].b as u32;
                assert_eq!(
                    rendered,
                    (*r as u32) << 16 | (*g as u32) << 8 | *b as u32,
                    "{}/ansi16[{i}]: palette changed with legibility off",
                    t.id
                );
            }
        }
    }

    #[test]
    fn dark_bright_black_is_lifted_past_the_floor() {
        // The reported bug: PSReadLine paints parameters/operators/members
        // with DarkGray (SGR 90 → bright black), and the dark builtins author
        // that slot far below the floor, so the tokens vanished on the dark
        // background. The palette the renderer sees must come out lifted.
        for (id, authored) in [
            ("dracula", 0x555555),
            ("harbor", 0x292929),
            ("one_dark_pro", 0x5c6370),
            ("rose_pine", 0x6e6a86),
        ] {
            let t = builtins().into_iter().find(|t| t.id == id).unwrap();
            let c = t.active_palette(true).ansi16[8];
            let rendered = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
            assert_ne!(rendered, authored, "{id}: brightBlack was left as authored");
            assert!(
                contrast(rendered, t.background_color()) >= TEXT_FLOOR - 0.01,
                "{id}: brightBlack {rendered:#08x} still only {:.2}:1 on the background",
                contrast(rendered, t.background_color())
            );
        }
        // Light themes hit the same wall from the other side: a pale
        // bright-black vanishes on a near-white background.
        let latte = builtins()
            .into_iter()
            .find(|t| t.id == "catppuccin_latte")
            .unwrap();
        let c = latte.active_palette(true).ansi16[8];
        let rendered = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
        assert_ne!(
            rendered, 0xacb0be,
            "catppuccin_latte: brightBlack was left as authored"
        );
        assert!(
            contrast(rendered, latte.background_color()) >= TEXT_FLOOR - 0.01,
            "catppuccin_latte: brightBlack {rendered:#08x} still only {:.2}:1",
            contrast(rendered, latte.background_color())
        );
    }

    #[test]
    fn selection_surface_stays_on_the_background_side() {
        for t in builtins() {
            let ap = t.active_palette(true);
            let sel = (ap.sel_bg.r as u32) << 16 | (ap.sel_bg.g as u32) << 8 | ap.sel_bg.b as u32;
            let to_bg = contrast(sel, t.background_color());
            let to_fg = contrast(sel, t.foreground);
            assert!(
                to_fg > to_bg,
                "{}: selection surface sits closer to the foreground",
                t.id
            );
        }
    }

    #[test]
    fn state_ladder_is_separable_on_every_surface() {
        for t in builtins() {
            let s = t.surfaces();
            // The side panels run their own lighter rungs;
            // `panel_state_ladder_…` holds those apart instead.
            for (name, sf) in [("window", s.window), ("popover", s.popover)] {
                let sel_base = contrast(sf.selected, sf.base);
                let sel_hover = contrast(sf.selected, sf.hover);
                let hover_base = contrast(sf.hover, sf.base);
                let cursor_sel = contrast(sf.cursor, sf.selected);
                assert!(
                    sel_base >= 1.25,
                    "{}/{name}: selected is only {sel_base:.2}:1 from the surface",
                    t.id
                );
                assert!(
                    sel_hover >= 1.08,
                    "{}/{name}: selected is only {sel_hover:.2}:1 from hover",
                    t.id
                );
                assert!(
                    cursor_sel >= 1.2,
                    "{}/{name}: cursor is only {cursor_sel:.2}:1 from the resting selection",
                    t.id
                );
                assert!(
                    hover_base >= 1.1,
                    "{}/{name}: hover is only {hover_base:.2}:1 from the surface",
                    t.id
                );
                assert!(
                    contrast(sf.pressed, sf.base) > sel_base,
                    "{}/{name}: pressed must read past selected",
                    t.id
                );
            }
        }
    }

    #[test]
    fn panel_state_ladder_is_separable_in_every_theme() {
        for t in builtins() {
            let s = t.surfaces();
            for (name, sf) in [("sidebar", s.sidebar), ("rail", s.rail)] {
                let hover_base = contrast(sf.hover, sf.base);
                let sel_base = contrast(sf.selected, sf.base);
                assert!(
                    hover_base >= state::PANEL_HOVER - 0.02,
                    "{}/{name}: hover is only {hover_base:.2}:1 from the surface",
                    t.id
                );
                assert!(
                    sel_base >= state::PANEL_SELECTED - 0.02 && sel_base > hover_base,
                    "{}/{name}: selected is only {sel_base:.2}:1 from the surface",
                    t.id
                );
                assert!(
                    contrast(sf.pressed, sf.base) > sel_base,
                    "{}/{name}: pressed must read past selected",
                    t.id
                );
            }
        }
    }

    #[test]
    fn state_ladder_is_theme_independent() {
        let ratios: Vec<f32> = builtins()
            .iter()
            .map(|t| {
                let w = t.surfaces().window;
                contrast(w.selected, w.base)
            })
            .collect();
        let (lo, hi) = ratios
            .iter()
            .fold((f32::MAX, 0.0f32), |(l, h), r| (l.min(*r), h.max(*r)));
        assert!(
            hi - lo < 0.05,
            "selected step drifts across themes: {lo:.2}:1 … {hi:.2}:1"
        );
        assert!(
            (lo - state::SELECTED).abs() < 0.05,
            "selected step {lo:.2}:1 missed its {:.2}:1 target",
            state::SELECTED
        );
    }

    #[test]
    fn dracula_selection_matches_the_signed_off_greys() {
        let dracula = builtins().into_iter().find(|t| t.id == "dracula").unwrap();
        let bg = dracula.background_color();
        let s = dracula.surfaces();
        // The resting rung is checked as `state::SELECTED` on the sidebar
        // fill it was signed off on — the old rail, 3% toward the foreground —
        // rather than on today's rail, which has since gone flat onto the
        // window fill. This pin is here to catch the constant drifting, not
        // that decision.
        let fg = legible_foreground(bg, dracula.foreground);
        let signed_off_rail = mix(bg, fg, 0.03);
        for (what, now, legacy) in [
            (
                "resting",
                raise(signed_off_rail, fg, state::SELECTED),
                mix(bg, dracula.foreground, 0.12),
            ),
            ("cursor", s.window.cursor, mix(bg, dracula.foreground, 0.17)),
        ] {
            assert!(
                contrast(now, legacy) < 1.05,
                "Dracula's {what} selection moved: {now:#08x} vs the tuned {legacy:#08x}"
            );
        }
    }

    /// CIE L*a*b* for a packed sRGB colour, D65.
    ///
    /// Contrast is a luminance ratio and says nothing about hue: two lanes can
    /// both clear 3:1 against the background and still be the same colour to
    /// look at. ΔE is the measure that catches that, and it needs Lab.
    fn lab(c: u32) -> (f32, f32, f32) {
        fn linear(v: u32) -> f32 {
            let s = v as f32 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        let (r, g, b) = (
            linear(c >> 16 & 0xff),
            linear(c >> 8 & 0xff),
            linear(c & 0xff),
        );
        // sRGB → XYZ, then normalised by the D65 white point.
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
        let f = |t: f32| {
            if t > 0.008856 {
                t.cbrt()
            } else {
                7.787 * t + 16.0 / 116.0
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
    }

    fn delta_e76(a: u32, b: u32) -> f32 {
        let (l1, a1, b1) = lab(a);
        let (l2, a2, b2) = lab(b);
        ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
    }

    #[test]
    fn lane_colours_clear_the_floor_on_every_surface() {
        for t in builtins() {
            let m = t.neutrals();
            let lanes = t.lanes();
            for (name, surface) in [
                ("background", m.background),
                ("sidebar", m.sidebar),
                ("popover", m.popover),
            ] {
                for (slot, ink) in lanes.ink.iter().enumerate() {
                    let ratio = contrast(*ink, surface);
                    assert!(
                        ratio >= ACCENT_FLOOR,
                        "{}/{name}: lane {slot} is only {ratio:.2}:1",
                        t.id
                    );
                }
                let ratio = contrast(lanes.overflow, surface);
                assert!(
                    ratio >= ACCENT_FLOOR,
                    "{}/{name}: the overflow lane is only {ratio:.2}:1",
                    t.id
                );
            }
        }
    }

    #[test]
    fn adjacent_lanes_are_never_the_same_colour() {
        // A just-noticeable difference is around 2.3. The floor is set far
        // above it because these are 1.5px lines a few pixels apart, not
        // patches side by side, and the eye is much worse at hairlines.
        const FLOOR: f32 = 12.0;
        for t in builtins() {
            let lanes = t.lanes();
            for slot in 0..LANE_SLOTS - 1 {
                let d = delta_e76(lanes.ink[slot], lanes.ink[slot + 1]);
                assert!(
                    d >= FLOOR,
                    "{}: lanes {slot} and {} are ΔE {d:.1} apart",
                    t.id,
                    slot + 1
                );
            }
        }
    }

    #[test]
    fn lane_colours_are_deterministic() {
        for t in builtins() {
            assert_eq!(
                t.lanes().ink,
                t.lanes().ink,
                "{}: lane derivation is not a pure function",
                t.id
            );
        }
    }

    /// The seeds were chosen so that no two neighbours share a hue family and
    /// red never sits beside green. Both are properties of the *order*, so a
    /// reshuffle has to fail here rather than only looking slightly worse.
    #[test]
    fn the_lane_seed_order_keeps_red_and_green_apart() {
        let seeds = [4usize, 3, 5, 2, 6, 1];
        let red = seeds.iter().position(|s| *s == 1).expect("red is a seed");
        let green = seeds.iter().position(|s| *s == 2).expect("green is a seed");
        assert!(
            red.abs_diff(green) > 1,
            "red and green ended up adjacent at slots {red} and {green}"
        );
        assert_eq!(red, LANE_SLOTS - 1, "red should be the last slot reached");
    }

    #[test]
    fn resting_labels_stay_readable() {
        for t in builtins() {
            let s = t.surfaces();
            for (name, sf) in [("window", s.window), ("popover", s.popover)] {
                let ratio = contrast(sf.text_resting, sf.base);
                assert!(
                    ratio >= 4.5,
                    "{}/{name}: resting label only {ratio:.2}:1",
                    t.id
                );
            }
        }
    }

    #[test]
    fn label_channel_is_readable_and_stepped() {
        for t in builtins() {
            let s = t.surfaces();
            for (name, sf) in [
                ("window", s.window),
                ("sidebar", s.sidebar),
                ("popover", s.popover),
            ] {
                let on_fill = contrast(sf.text_selected, sf.selected);
                assert!(
                    on_fill >= 4.5,
                    "{}/{name}: selected label only {on_fill:.2}:1 on its own fill",
                    t.id
                );
                let step = contrast(sf.text_selected, sf.text_resting);
                assert!(
                    step >= 1.35,
                    "{}/{name}: label step is only {step:.2}:1 — the channel says nothing",
                    t.id
                );
            }
        }
    }

    #[test]
    fn selected_label_is_stepped_off_the_resting_one() {
        let (base, fill, fg) = (0x2f333b, 0x40454d, 0xabb2bf);
        let resting = dim(fg, base, state::TEXT_RESTING);

        let plain = ink_on(fill, fg, state::TEXT_RESTING);
        assert!(
            contrast(plain, resting) < state::TEXT_STEP,
            "the case this floor exists for no longer collapses — pick another"
        );

        let ink = stepped_ink(fill, base, fg, resting);
        assert!(
            contrast(ink, resting) >= state::TEXT_STEP - 0.01,
            "stepped to {ink:#08x}, still only {:.2}:1 off the resting label",
            contrast(ink, resting)
        );
        assert!(
            contrast(ink, fill) >= state::TEXT_RESTING - 0.01,
            "stepping cost the label its fill: {:.2}:1 on {fill:#08x}",
            contrast(ink, fill)
        );
    }

    #[test]
    fn switch_tracks_and_knob_stay_legible() {
        for t in builtins() {
            let m = t.neutrals();
            let unchecked = t.surfaces().window.selected;
            let checked = m.accent;
            let knob = if is_lighter(m.background, m.foreground) {
                m.background
            } else {
                m.foreground
            };
            assert!(
                contrast(knob, unchecked) >= 1.25,
                "{}: knob {knob:#08x} lost on the unchecked track {unchecked:#08x}",
                t.id
            );
            assert!(
                contrast(knob, checked) >= 1.25,
                "{}: knob {knob:#08x} lost on the checked track {checked:#08x}",
                t.id
            );
            assert!(
                contrast(checked, unchecked) >= 1.3,
                "{}: checked and unchecked tracks are {:.2}:1 apart",
                t.id,
                contrast(checked, unchecked)
            );
        }
    }

    #[test]
    fn semantic_colors_clear_their_floors() {
        for t in builtins() {
            let bg = t.background_color();
            let m = t.neutrals();
            let s = t.semantics();
            for (name, c) in [
                ("danger", s.danger),
                ("warning", s.warning),
                ("success", s.success),
                ("info", s.info),
                ("link", s.link),
            ] {
                // Every neutral fill the ink can land on, not just the window:
                // a failure notice is usually read inside a popover.
                for (surface, fill) in [
                    ("background", bg),
                    ("sidebar", m.sidebar),
                    ("popover", m.popover),
                ] {
                    assert!(
                        contrast(c.ink, fill) >= TEXT_FLOOR - 0.01,
                        "{}/{name}: ink {:#08x} only {:.2}:1 on the {surface}",
                        t.id,
                        c.ink,
                        contrast(c.ink, fill)
                    );
                    assert!(
                        contrast(c.fill, fill) >= ACCENT_FLOOR - 0.01,
                        "{}/{name}: fill {:#08x} only {:.2}:1 on the {surface}",
                        t.id,
                        c.fill,
                        contrast(c.fill, fill)
                    );
                }
                assert!(
                    contrast(c.on_fill, c.fill) >= TEXT_FLOOR - 0.01,
                    "{}/{name}: text on its own fill is only {:.2}:1",
                    t.id,
                    contrast(c.on_fill, c.fill)
                );
            }
            for (a, b, pair) in [
                (s.danger.ink, s.success.ink, "danger/success"),
                (s.danger.ink, s.warning.ink, "danger/warning"),
                (s.success.ink, s.warning.ink, "success/warning"),
            ] {
                assert!(
                    channel_distance(a, b) >= 40,
                    "{}: {pair} collapsed to nearly the same colour ({a:#08x} vs {b:#08x})",
                    t.id
                );
            }
        }
    }

    #[test]
    fn agent_status_semantics_clear_floors_on_rail_and_window() {
        use crate::core::cli_agent::AgentStatus;
        for t in builtins() {
            let sem = t.semantics();
            let sf = t.surfaces();
            assert!(
                sem.for_agent_status(AgentStatus::Idle).is_none(),
                "{}: Idle should carry no status role",
                t.id
            );
            for (label, status) in [
                ("Working", AgentStatus::Working),
                ("Waiting", AgentStatus::Waiting),
                ("Done", AgentStatus::Done),
            ] {
                let role = sem
                    .for_agent_status(status)
                    .unwrap_or_else(|| panic!("{}: {label} missing semantic role", t.id));
                for (surface_name, surface) in [
                    ("rail.base", sf.rail.base),
                    ("rail.hover", sf.rail.hover),
                    ("rail.selected", sf.rail.selected),
                    ("window.base", sf.window.base),
                    ("window.hover", sf.window.hover),
                    ("window.selected", sf.window.selected),
                ] {
                    let ink_ratio = contrast(role.ink, surface);
                    assert!(
                        ink_ratio >= ACCENT_FLOOR - 0.01,
                        "{}/{label}: ink {:#08x} only {ink_ratio:.2}:1 on {surface_name} {surface:#08x}",
                        t.id,
                        role.ink
                    );
                }
                for (surface_name, surface) in
                    [("rail.base", sf.rail.base), ("window.base", sf.window.base)]
                {
                    let ink_ratio = contrast(role.ink, surface);
                    assert!(
                        ink_ratio >= TEXT_FLOOR - 0.01,
                        "{}/{label}: text ink {:#08x} only {ink_ratio:.2}:1 on {surface_name} {surface:#08x}",
                        t.id,
                        role.ink
                    );
                    let fill_ratio = contrast(role.fill, surface);
                    assert!(
                        fill_ratio >= ACCENT_FLOOR - 0.01,
                        "{}/{label}: fill {:#08x} only {fill_ratio:.2}:1 on {surface_name} {surface:#08x}",
                        t.id,
                        role.fill
                    );
                }
                let on_fill_ratio = contrast(role.on_fill, role.fill);
                assert!(
                    on_fill_ratio >= TEXT_FLOOR - 0.01,
                    "{}/{label}: on_fill {:#08x} only {on_fill_ratio:.2}:1 on fill {:#08x}",
                    t.id,
                    role.on_fill,
                    role.fill
                );
            }
        }
    }

    #[test]
    fn semantic_roles_are_independent_of_terminal_ansi() {
        let mut theme = builtins().into_iter().find(|t| t.id == "dracula").unwrap();
        let before = theme.semantics();
        theme.ansi16.swap(1, 2);
        theme.ansi16.swap(3, 6);
        let after = theme.semantics();
        assert_eq!(before.danger.ink, after.danger.ink);
        assert_eq!(before.success.ink, after.success.ink);
        assert_eq!(before.warning.ink, after.warning.ink);
        assert_eq!(before.link.ink, after.link.ink);
        assert_ne!(
            theme.active_palette(false).ansi16[1],
            theme.active_palette(false).ansi16[2]
        );
    }

    #[test]
    fn interaction_roles_have_readable_labels_in_all_states() {
        for theme in builtins() {
            let c = theme.interactions();
            for fill in [c.primary.fill, c.primary_hover, c.primary_pressed] {
                assert!(
                    contrast(fill, c.primary.on_fill) >= 4.5 - 0.01,
                    "{} primary",
                    theme.id
                );
            }
            assert!(
                contrast(c.choice, c.choice_ink) >= 4.5,
                "{} choice",
                theme.id
            );
            assert_ne!(
                c.input_border, c.choice,
                "{} field boundaries must not use selected-row fills",
                theme.id
            );
        }
    }

    #[test]
    fn captions_are_readable_on_every_surface_they_are_used_on() {
        for theme in builtins() {
            let m = theme.neutrals();
            let s = theme.surfaces();
            for surface in [
                m.background,
                m.sidebar,
                m.rail,
                m.popover,
                s.rail.hover,
                s.rail.selected,
            ] {
                assert!(
                    contrast(m.muted_foreground, surface) >= TEXT_FLOOR,
                    "{}: caption {:#08x} on {surface:#08x}",
                    theme.id,
                    m.muted_foreground,
                );
            }
        }
    }

    #[test]
    fn sidebar_title_is_stepped_off_its_caption() {
        // The three rungs of a sidebar row — title, branch line, group
        // header — all sat at the same 4.5:1 grey once; this is the guard
        // against that column flattening again. The caption is dimmed on the
        // window and painted on the sidebar, so measure it where it lands.
        for t in builtins() {
            let m = t.neutrals();
            let step = contrast(m.sidebar_fg, m.muted_foreground);
            assert!(
                step >= state::TEXT_STEP - 0.01,
                "{}: title {:#08x} is only {step:.2}:1 off the caption {:#08x}",
                t.id,
                m.sidebar_fg,
                m.muted_foreground
            );
        }
    }

    #[test]
    fn resting_semantic_ink_keeps_the_text_floor() {
        for t in builtins() {
            let m = t.neutrals();
            let sem = t.semantics();
            let beside: Hsla = gpui::rgb(m.muted_foreground).into();
            for (name, ink) in [("success", sem.success.ink), ("danger", sem.danger.ink)] {
                for (surface_name, surface) in [
                    ("window", m.background),
                    ("sidebar", m.sidebar),
                    ("popover", m.popover),
                ] {
                    let resting = pack(resting_ink(
                        gpui::rgb(ink).into(),
                        beside,
                        gpui::rgb(surface).into(),
                    ));
                    let ratio = contrast(resting, surface);
                    assert!(
                        ratio >= TEXT_FLOOR - 0.02,
                        "{}/{surface_name}: resting {name} {resting:#08x} is only {ratio:.2}:1",
                        t.id
                    );
                }
            }
        }
    }

    #[test]
    fn sidebar_text_reads_on_the_fill_it_is_painted_on() {
        for t in builtins() {
            let m = t.neutrals();
            let ratio = contrast(m.sidebar_fg, m.sidebar);
            assert!(
                ratio >= TITLE_FLOOR - 0.01,
                "{}: sidebar text {:#08x} is only {ratio:.2}:1 on the sidebar fill {:#08x}",
                t.id,
                m.sidebar_fg,
                m.sidebar
            );
            let ratio = contrast(m.rail_fg, m.rail);
            assert!(
                ratio >= TITLE_FLOOR - 0.01,
                "{}: rail text {:#08x} is only {ratio:.2}:1 on the rail fill {:#08x}",
                t.id,
                m.rail_fg,
                m.rail
            );
        }
    }

    #[test]
    fn the_rail_is_tinted_and_the_right_panel_is_not() {
        for t in builtins() {
            let m = t.neutrals();
            assert_eq!(m.sidebar, m.background, "{}: right panel fill", t.id);
            assert_ne!(m.rail, m.background, "{}: rail fill", t.id);
            let s = t.surfaces().rail;
            assert!(
                contrast(s.selected, s.text_selected) >= TEXT_FLOOR,
                "{}: current rail row",
                t.id
            );
        }
        let light = builtins().into_iter().find(|t| t.id == "light").unwrap();
        assert!(channel_distance(light.neutrals().rail, 0xf5f5f3) <= 1);
        let dark = builtins().into_iter().find(|t| t.id == "dark").unwrap();
        assert!(channel_distance(dark.neutrals().rail, 0x1e1e20) <= 1);
    }

    #[test]
    fn carets_stay_visible_on_the_background() {
        for t in builtins() {
            let m = t.neutrals();
            let ratio = contrast(m.caret, m.background);
            assert!(
                ratio >= ACCENT_FLOOR - 0.01,
                "{}: caret {:#08x} is only {ratio:.2}:1 on the background",
                t.id,
                m.caret
            );
        }
        // The default theme used to ship an orange caret that read at 2.07:1
        // on pure white. It now takes the accent, like the focus ring.
        let light = builtins().into_iter().find(|t| t.id == DEFAULT_ID).unwrap();
        assert_eq!(light.neutrals().caret, light.neutrals().accent);
    }

    #[test]
    fn hairlines_are_worth_the_same_in_every_theme() {
        for t in builtins() {
            let m = t.neutrals();
            for (tier, ink, floor) in [
                ("border", m.border, BORDER_FLOOR),
                ("divider", m.divider, DIVIDER_FLOOR),
            ] {
                for (name, surface) in [
                    ("background", m.background),
                    ("sidebar", m.sidebar),
                    ("popover", m.popover),
                ] {
                    let ratio = contrast(ink, surface);
                    assert!(
                        ratio >= floor - 0.01,
                        "{}: {tier} {ink:#08x} is only {ratio:.2}:1 on the {name}",
                        t.id
                    );
                    // A floor, not a target — a hairline that shouts is worse
                    // than one that whispers.
                    assert!(
                        ratio <= 2.2,
                        "{}: {tier} {ink:#08x} is {ratio:.2}:1 on the {name} and reads as a frame",
                        t.id
                    );
                }
            }
        }
    }

    /// The two tiers have to stay apart, or the split is decoration. A divider
    /// that lands on the same value as the border is the state this replaced.
    #[test]
    fn a_divider_is_lighter_than_a_border_in_every_theme() {
        for t in builtins() {
            let m = t.neutrals();
            assert!(
                contrast(m.divider, m.sidebar) < contrast(m.border, m.sidebar),
                "{}: divider {:#08x} is not lighter than border {:#08x}",
                t.id,
                m.divider,
                m.border
            );
        }
    }

    #[test]
    fn accents_are_conditioned_to_carry_ink() {
        for t in builtins() {
            let bg = t.background_color();
            let a = t.neutrals().accent;
            let ratio = contrast(a, bg);
            assert!(
                ratio >= ACCENT_FLOOR - 0.01,
                "{}: accent {a:#08x} only {ratio:.2}:1 on the background",
                t.id
            );
        }
        let rose = builtins()
            .into_iter()
            .find(|t| t.id == "rose_pine")
            .unwrap();
        assert_eq!(rose.neutrals().accent, rose.accent);
    }

    #[test]
    fn a_match_wash_is_visible_on_a_white_theme_and_a_black_one() {
        // The old fixed alpha: 0.32 of a tint 24% off the background.
        let faint = |bg: u32, fg: u32| {
            let tint = mix(bg, fg, 0.24);
            contrast(mix(bg, tint, 0.32), bg)
        };
        for (bg, fg) in [(0xffffff, 0x111111), (0x111111, 0xffffff)] {
            assert!(
                faint(bg, fg) < 1.25,
                "the old wash was {:.2}:1 on {bg:06x} — that is what made it vanish",
                faint(bg, fg)
            );
            let tint = mix(bg, fg, 0.24);
            let (hit, current) = match_wash_targets(bg, fg);
            let w = wash(bg, tint, hit);
            assert!(
                contrast(w, bg) >= hit - 0.02,
                "{:.2}:1 on {bg:06x}",
                contrast(w, bg)
            );
            let cur = wash(bg, tint, current);
            assert!(
                contrast(cur, bg) > contrast(w, bg),
                "the current match has to stand out from the rest"
            );
        }
    }

    #[test]
    fn a_low_contrast_theme_gets_a_gentler_wash_than_a_high_contrast_one() {
        let (soft_hit, soft_cur) = match_wash_targets(0x282c34, 0xabb2bf);
        let (hard_hit, hard_cur) = match_wash_targets(0x000000, 0xffffff);
        assert!(
            soft_cur < hard_cur && soft_hit < hard_hit,
            "the wash spends the theme's own contrast budget, so it has to \
             scale with it: {soft_cur:.2} vs {hard_cur:.2}"
        );
        assert!(
            soft_hit >= 1.45,
            "even the gentle end has to beat the flat constant it replaced"
        );
        assert!(hard_cur <= WASH_CEILING, "a highlight, not a solid block");
    }

    #[test]
    fn a_match_wash_leaves_the_glyph_painted_on_top_of_it_readable() {
        // The wash is opaque and the text is drawn over it, so every step up in
        // visibility is a step down in the contrast the glyph has left. This is
        // the ceiling both targets are chosen against — on every builtin, and
        // in the accent the terminal actually washes with.
        for t in builtins() {
            let n = t.neutrals();
            let (hit_t, cur_t) = match_wash_targets(n.background, n.foreground);
            let hit = wash(n.background, n.accent, hit_t);
            let cur = wash(n.background, n.accent, cur_t);
            for (label, fill) in [("hit", hit), ("current", cur)] {
                // `wash` bisects to *at least* its target and 8-bit channels do
                // not divide evenly, so a fill can sit a hair past where the
                // target asked for it — and the glyph pays that hair.
                assert!(
                    contrast(n.foreground, fill) >= GLYPH_FLOOR - 0.05,
                    "{}: text on a {label} is {:.2}:1",
                    t.id,
                    contrast(n.foreground, fill)
                );
            }
            assert!(
                contrast(hit, n.background) > 1.5,
                "{}: a hit only shifts {:.2}:1 off the background — a hairline",
                t.id,
                contrast(hit, n.background)
            );
            assert!(
                contrast(cur, hit) >= 1.2,
                "{}: the current match only reads {:.2}:1 apart from the rest",
                t.id,
                contrast(cur, hit)
            );
        }
    }

    #[test]
    fn a_wash_whose_tint_cannot_reach_the_target_still_lands_on_something_legible() {
        // A theme whose selection colour is its background: no alpha gets there.
        let (hit, _) = match_wash_targets(0xffffff, 0x111111);
        let w = wash(0xffffff, 0xfefefe, hit);
        assert!(contrast(w, 0xffffff) >= hit - 0.02, "{w:06x}");
    }

    #[test]
    fn contrast_bisection_hits_its_target() {
        const SLACK: f32 = 0.05;
        let f = raise(0x000000, 0xffffff, 2.0);
        assert!((2.0..2.0 + SLACK).contains(&contrast(f, 0x000000)));
        let f = raise(0xffffff, 0x000000, 2.0);
        assert!((2.0..2.0 + SLACK).contains(&contrast(f, 0xffffff)));
        let d = dim(0xffffff, 0x000000, 4.5);
        assert!((contrast(d, 0x000000) - 4.5).abs() < SLACK);
        assert_eq!(raise(0x000000, 0x808080, 21.0), 0x808080);
    }

    #[test]
    fn semantic_conditioning_survives_a_midtone_background() {
        let bg = 0x808080;
        for seed in [0xff5555u32, 0x50fa7b, 0xf1fa8c, 0x8be9fd] {
            let ink = legible_ink(bg, seed, TEXT_FLOOR);
            assert!(
                contrast(ink, bg) >= TEXT_FLOOR - 0.01,
                "{seed:#08x} conditioned to {ink:#08x}, only {:.2}:1 on a midtone ground",
                contrast(ink, bg)
            );
        }
    }

    #[test]
    fn legible_foreground_rescues_unreadable_text() {
        assert_eq!(legible_foreground(0xffffff, 0xeeeeee), 0x000000);
        assert_eq!(legible_foreground(0xffffff, 0x111111), 0x111111);
        assert_eq!(legible_foreground(0x000000, 0x222222), 0xffffff);
    }

    #[test]
    fn parse_hex_accepts_optional_hash_and_rejects_junk() {
        assert_eq!(parse_hex("#123456").unwrap(), 0x123456);
        assert_eq!(parse_hex("abcdef").unwrap(), 0xabcdef);
        assert!(parse_hex("#fff").is_err());
        assert!(parse_hex("nope!!").is_err());
    }

    #[test]
    fn yaml_theme_parses_normal_then_bright() {
        let yaml = r##"
background: "#101010"
foreground: "#e0e0e0"
accent: "#ff8800"
ansi:
  normal: ["#000000","#111111","#222222","#333333","#444444","#555555","#666666","#777777"]
  bright: ["#888888","#999999","#aaaaaa","#bbbbbb","#cccccc","#dddddd","#eeeeee","#ffffff"]
"##;
        let file: ThemeFile = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(file.background, FillFile::Solid(_)));
        let bg = file.background.into_fill().unwrap().color();
        assert_eq!(bg, 0x101010);
        assert_eq!(parse_rgb(&file.ansi.normal[0]).unwrap(), (0, 0, 0));
        assert_eq!(parse_rgb(&file.ansi.bright[7]).unwrap(), (0xff, 0xff, 0xff));
    }

    #[test]
    fn yaml_gradient_background_parses() {
        let file: ThemeFile = serde_yaml::from_str(
            r##"
background: { top: "#001122", bottom: "#334455" }
foreground: "#ffffff"
accent: "#ff0000"
ansi:
  normal: ["#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000"]
  bright: ["#ffffff","#ffffff","#ffffff","#ffffff","#ffffff","#ffffff","#ffffff","#ffffff"]
"##,
        )
        .unwrap();
        let fill = file.background.into_fill().unwrap();
        assert_eq!(
            fill,
            Fill::Vertical {
                top: 0x001122,
                bottom: 0x334455
            }
        );
        assert_eq!(fill.color(), 0x001122);
    }

    #[test]
    fn to_yaml_round_trips_window_and_image_fields() {
        let mut theme = builtins().into_iter().next().unwrap();
        theme.background = Fill::Vertical {
            top: 0x001122,
            bottom: 0x334455,
        };
        theme.opacity = Some(0.85);
        theme.blur = true;
        theme.image = Some(Image {
            path: PathBuf::from("/pictures/koi.jpg"),
            opacity: 0.4,
        });
        let file: ThemeFile = serde_yaml::from_str(&to_yaml(&theme)).unwrap();
        assert_eq!(
            file.background.into_fill().unwrap(),
            theme.background,
            "gradient background lost"
        );
        assert_eq!(file.opacity, Some(0.85));
        assert!(file.blur);
        let img = file.background_image.expect("image field lost");
        assert_eq!(img.path, "/pictures/koi.jpg");
        assert_eq!(img.opacity, 0.4);
    }

    #[test]
    fn iterm_theme_accepts_integer_components_and_no_alpha() {
        let entry = |key: String, r: &str| {
            format!(
                "<key>{key}</key><dict><key>Red Component</key>{r}\
                 <key>Green Component</key><real>0.5</real>\
                 <key>Blue Component</key><integer>0</integer></dict>"
            )
        };
        let mut body: String = (0..16)
            .map(|i| entry(format!("Ansi {i} Color"), "<integer>1</integer>"))
            .collect();
        body += &entry("Background Color".into(), "<integer>0</integer>");
        body += &entry("Foreground Color".into(), "<real>1.0</real>");
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <plist version=\"1.0\"><dict>{body}</dict></plist>"
        );
        let path =
            std::env::temp_dir().join(format!("tty59-int-{}.itermcolors", std::process::id()));
        std::fs::write(&path, xml).unwrap();
        let theme = load_iterm_theme(&path);
        let _ = std::fs::remove_file(&path);
        let theme = theme.unwrap();
        assert_eq!(theme.ansi16[2], (0xff, 0x80, 0x00));
        assert_eq!(theme.background_color(), 0x008000);
        assert_eq!(theme.foreground, 0xff8000);
    }

    #[test]
    fn id_and_name_titlecases_the_stem() {
        let (id, name) = id_and_name(std::path::Path::new("/x/solarized_dark.yaml"));
        assert_eq!(id, "solarized_dark");
        assert_eq!(name, "Solarized Dark");
    }

    #[test]
    fn theme_names_stay_english_under_a_translated_gui() {
        crate::ui::i18n::set_locale("zh-CN");
        // A stem of only separators leaves nothing to title-case.
        let (_, name) = id_and_name(std::path::Path::new("/x/_.yaml"));
        assert_eq!(name, "Theme");
        crate::ui::i18n::set_locale("en");
    }

    #[test]
    fn mix_blends_channels() {
        assert_eq!(mix(0x000000, 0xffffff, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 1.0), 0xffffff);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
    }

    /// The external contract the sweeps hold the derivations to, written out
    /// rather than read from the production constants: WCAG AA for text, and
    /// 3:1 for marks and non-text UI.
    const AA_TEXT: f32 = 4.5;
    /// The headroom a corrected text role must land on, written out here on
    /// purpose: asserting against the production constant would pass if that
    /// constant were ever lowered.
    const AA_CORRECTED: f32 = 4.6;
    const AA_MARK: f32 = 3.0;

    fn sweep_themes() -> Vec<Theme> {
        let mut themes = builtins();
        themes.extend(generated_theme_grid());
        themes
    }

    fn assert_no_misses(what: &str, misses: Vec<String>) {
        assert!(
            misses.is_empty(),
            "{what}: {} misses, first 12:\n{}",
            misses.len(),
            misses
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// One ink on one fill it is painted on, and the floor it must clear.
    struct Check {
        role: String,
        fill: u32,
        ink: u32,
        floor: f32,
    }

    impl Check {
        fn ratio(&self) -> f32 {
            contrast(self.ink, self.fill)
        }

        fn holds(&self) -> bool {
            self.ratio() >= self.floor
        }
    }

    /// Every ink role the chrome derivations feed, measured on the fills it
    /// is painted on. No role is skipped for any theme.
    fn role_checks(t: &Theme) -> Vec<Check> {
        let r = t.resolve();
        let (m, s) = (&r.m, &r.sf);
        let mut checks = Vec::new();
        let mut add = |role: &str, ink: u32, fills: &[u32], floor: f32| {
            checks.extend(fills.iter().map(|&fill| Check {
                role: role.to_string(),
                fill,
                ink,
                floor,
            }));
        };
        add(
            "caption",
            m.muted_foreground,
            &[
                m.background,
                m.sidebar,
                m.rail,
                m.popover,
                s.rail.hover,
                s.rail.selected,
            ],
            AA_TEXT,
        );
        // An Inbox row's reason and meta lines are muted_foreground on the sidebar
        // rungs (the row is painted from `sf.sidebar`, not the window's rungs),
        // which the caption set above (rail rungs only) does not cover.
        add(
            "inbox reason meta",
            m.muted_foreground,
            &[s.sidebar.base, s.sidebar.hover, s.sidebar.selected],
            AA_TEXT,
        );
        let sem = t.semantics_with(m);
        let resting = [m.background, m.sidebar, m.rail, m.popover];
        let every_rung = [
            s.rail.base,
            s.rail.hover,
            s.rail.selected,
            s.window.base,
            s.window.hover,
            s.window.selected,
        ];
        for (label, role) in [
            ("danger", sem.danger),
            ("warning", sem.warning),
            ("success", sem.success),
            ("info", sem.info),
        ] {
            add(
                &format!("status {label} ink text"),
                role.ink,
                &resting,
                AA_TEXT,
            );
            add(
                &format!("status {label} fill mark"),
                role.fill,
                &resting,
                AA_MARK,
            );
            add(
                &format!("status {label} ink mark"),
                role.ink,
                &every_rung,
                AA_MARK,
            );
            add(
                &format!("status {label} on_fill"),
                role.on_fill,
                &[role.fill],
                AA_TEXT,
            );
        }
        let settings = t.settings_ink_from(&r);
        add("settings sub", settings.sub, &r.settings_text, AA_TEXT);
        add(
            "settings switch_off",
            settings.switch_off,
            &r.settings_marks,
            AA_MARK,
        );
        add("settings glyph", settings.glyph, &r.settings_glyph, AA_MARK);
        for (what, ink) in [
            ("danger", settings.danger),
            ("warn_text", settings.warn_text),
            ("ok", settings.ok),
        ] {
            add(
                &format!("settings {what} text"),
                ink,
                &r.settings_status,
                AA_TEXT,
            );
        }
        // The warning dot (agents status, shortcut conflict) and the conflict
        // ring: on the page, and in a shortcut conflict on the k04 row wash
        // behind the row and its note.
        add(
            "settings warn mark",
            settings.warn,
            &r.settings_warn,
            AA_MARK,
        );
        let chrome = t.chrome_ink_from(&r);
        let popover = [s.popover.base, s.popover.hover, s.popover.selected];
        let panel = [
            s.sidebar.base,
            s.sidebar.hover,
            s.sidebar.selected,
            s.rail.base,
            s.rail.hover,
            s.rail.selected,
        ];
        add(
            "chrome selected_meta",
            chrome.selected_meta,
            &[s.popover.selected],
            AA_TEXT,
        );
        add("chrome quiet_name", chrome.quiet_name, &popover, AA_TEXT);
        let unpicked = [s.popover.base, s.popover.hover];
        add(
            "chrome unpicked_title",
            chrome.unpicked_title,
            &unpicked,
            AA_TEXT,
        );
        add(
            "chrome unpicked_meta",
            chrome.unpicked_meta,
            &unpicked,
            AA_TEXT,
        );
        add(
            "chrome avatar_ink",
            chrome.avatar_ink,
            &[m.secondary],
            AA_TEXT,
        );
        add(
            "chrome keycap_ink",
            chrome.keycap_ink,
            &[chrome.keycap_fill],
            AA_TEXT,
        );
        add(
            "chrome grip_idle",
            chrome.grip_idle,
            &[s.window.base],
            AA_MARK,
        );
        add(
            "chrome search_title",
            chrome.search_title,
            &popover,
            AA_TEXT,
        );
        add("chrome panel_title", chrome.panel_title, &panel, AA_TEXT);
        add(
            "chrome port_chip_ink",
            chrome.port_chip_ink,
            &r.port_chip,
            AA_TEXT,
        );
        add(
            "chrome port_chip_hover_ink",
            chrome.port_chip_hover_ink,
            &r.port_chip,
            AA_TEXT,
        );
        for (name, sf) in [
            ("window", s.window),
            ("popover", s.popover),
            ("sidebar", s.sidebar),
            ("rail", s.rail),
        ] {
            add(
                &format!("title {name}"),
                sf.text_selected,
                &[sf.selected],
                AA_TEXT,
            );
        }
        checks
    }

    fn role_misses(t: &Theme) -> Vec<String> {
        role_checks(t)
            .into_iter()
            .filter(|c| !c.holds())
            .map(|c| {
                format!(
                    "{}: {} {:#08x} on {:#08x} is {:.2}:1 (floor {})",
                    t.id,
                    c.role,
                    c.ink,
                    c.fill,
                    c.ratio(),
                    c.floor
                )
            })
            .collect()
    }

    /// Strict, with no theme and no role excluded: every ink clears its floor
    /// on every fill it is painted on, in the built-ins and across the whole
    /// generated grid, including the palettes whose chrome fills invert.
    #[test]
    fn every_role_holds_its_floor_in_every_builtin_and_generated_theme() {
        let misses: Vec<String> = sweep_themes().iter().flat_map(role_misses).collect();
        assert_no_misses("role floors", misses);
    }

    fn crossover_theme(background: u32, foreground: u32) -> Theme {
        Theme {
            id: format!("crossover-{background:06x}"),
            name: "Crossover".into(),
            dark: is_dark(background),
            background: Fill::Solid(background),
            foreground,
            accent: 0x1f6bf0,
            caret: None,
            selection: None,
            opacity: None,
            blur: false,
            image: None,
            ansi16: BUILTINS[0].ansi16,
            path: None,
        }
    }

    /// The chrome pole is the authored foreground on every built-in, and
    /// every fill derived from it is what the pre-pole formulas gave: the
    /// built-ins are unchanged by the policy.
    #[test]
    fn builtin_chrome_is_derived_from_the_authored_foreground_as_before() {
        for t in builtins() {
            let bg = t.background_color();
            let fg = legible_foreground(bg, t.foreground);
            let r = t.resolve();
            assert_eq!(r.pole, fg, "{}: pole", t.id);
            assert_eq!(r.m.rail, mix(bg, fg, RAIL_TINT), "{}: rail", t.id);
            assert_eq!(
                r.m.popover,
                if t.dark { mix(bg, fg, 0.06) } else { bg },
                "{}: popover",
                t.id
            );
            assert_eq!(
                (r.sf.rail.hover, r.sf.rail.selected),
                panel_rungs(r.m.rail, fg),
                "{}: rail rungs",
                t.id
            );
            assert_eq!(
                (r.sf.sidebar.hover, r.sf.sidebar.selected),
                panel_rungs(bg, fg),
                "{}: sidebar rungs",
                t.id
            );
            for (name, base, sf) in [
                ("window", bg, r.sf.window),
                ("popover", r.m.popover, r.sf.popover),
            ] {
                let hover = raise(base, fg, state::HOVER);
                assert_eq!(sf.hover, hover, "{}/{name}: hover", t.id);
                assert_eq!(
                    sf.selected,
                    legible_rung(raise(base, fg, state::SELECTED), hover, base, fg),
                    "{}/{name}: selected",
                    t.id
                );
                assert_eq!(
                    sf.pressed,
                    raise(base, fg, state::PRESSED),
                    "{}/{name}: pressed",
                    t.id
                );
                assert_eq!(
                    sf.cursor,
                    raise(base, fg, state::CURSOR),
                    "{}/{name}: cursor",
                    t.id
                );
            }
            assert_eq!(r.m.secondary, mix(bg, fg, 0.09), "{}: secondary", t.id);
            assert_eq!(r.m.muted, mix(bg, fg, 0.06), "{}: muted", t.id);
            if t.selection.is_none() {
                assert_eq!(r.m.selection, mix(bg, fg, 0.20), "{}: selection", t.id);
            }
            assert_eq!(
                t.chrome_ink_from(&r).keycap_fill,
                mix(r.m.popover, fg, 0.06),
                "{}: keycap fill",
                t.id
            );
            assert_eq!(r.raise, settings_wash::RAISE, "{}: raise", t.id);
            assert_eq!(r.menu_lift, settings_wash::MENU_LIFT_DARK, "{}: lift", t.id);
        }
    }

    /// A palette on the white/black crossover: stepping toward its ink walks
    /// the chrome fills out from under every ink (black on #777777 is 4.68:1,
    /// but the 3% rail tint is 4.43:1, a dark popover 4.24:1, the rungs and
    /// washes lower still). The pole inverts, so every one of those fills
    /// steps away from the ink, the authored base and terminal colours are
    /// untouched, and every role holds.
    #[test]
    fn a_crossover_palette_inverts_every_chrome_fill_and_every_role_holds() {
        let t = crossover_theme(0x777777, 0x000000);
        assert!(
            !t.derive(0x000000).shares_one_ink(),
            "stepping toward the authored ink cannot serve this palette"
        );
        let r = t.resolve();
        assert_eq!(r.pole, 0xffffff);
        assert!(r.shares_one_ink());

        assert_eq!(t.background_color(), 0x777777);
        assert_eq!(r.m.background, 0x777777);
        assert_eq!(r.m.foreground, 0x000000);
        let sel_bg = t.active_palette(true).sel_bg;
        assert_eq!(
            (sel_bg.r as u32) << 16 | (sel_bg.g as u32) << 8 | sel_bg.b as u32,
            mix(0x777777, 0x000000, 0.24),
            "the terminal selection is still derived from the authored ink"
        );

        let base = lightness(0x777777);
        let wash_fills = [r.settings_text[1], r.settings_text[2]];
        for (name, fill) in [
            ("rail", r.m.rail),
            ("popover", r.m.popover),
            ("rail.hover", r.sf.rail.hover),
            ("rail.selected", r.sf.rail.selected),
            ("sidebar.hover", r.sf.sidebar.hover),
            ("sidebar.selected", r.sf.sidebar.selected),
            ("window.hover", r.sf.window.hover),
            ("window.selected", r.sf.window.selected),
            ("popover.hover", r.sf.popover.hover),
            ("popover.selected", r.sf.popover.selected),
            ("secondary", r.m.secondary),
            ("muted", r.m.muted),
            ("keycap fill", t.chrome_ink_from(&r).keycap_fill),
            ("row wash", wash_fills[0]),
            ("keycap wash", wash_fills[1]),
            ("port chip", r.port_chip[0]),
        ] {
            assert!(
                lightness(fill) > base,
                "{name} {fill:#08x} should step away from the black ink"
            );
        }

        let checks = role_checks(&t);
        let measured: Vec<&str> = checks.iter().map(|c| c.role.as_str()).collect();
        for role in [
            "caption",
            "inbox reason meta",
            "settings sub",
            "settings switch_off",
            "settings warn mark",
            "chrome selected_meta",
            "chrome quiet_name",
            "chrome unpicked_title",
            "chrome unpicked_meta",
            "chrome avatar_ink",
            "chrome keycap_ink",
            "chrome grip_idle",
            "chrome search_title",
            "chrome panel_title",
            "chrome port_chip_ink",
            "chrome port_chip_hover_ink",
            "title window",
            "title popover",
            "title sidebar",
            "title rail",
        ]
        .into_iter()
        .map(String::from)
        .chain(
            ["danger", "warning", "success", "info"]
                .into_iter()
                .flat_map(|label| {
                    ["ink text", "fill mark", "ink mark", "on_fill"]
                        .into_iter()
                        .map(move |what| format!("status {label} {what}"))
                }),
        ) {
            assert!(measured.contains(&role.as_str()), "{role} was not measured");
        }
        assert_no_misses("crossover roles", role_misses(&t));
    }

    /// The criterion is the fills, not the colour name: a black ink a few
    /// greys above the band keeps the authored pole, a white ink inside the
    /// mirror band inverts to black, and both hold every role.
    #[test]
    fn the_pole_inverts_only_where_the_authored_ink_cannot_serve_its_fills() {
        let above = crossover_theme(0x909090, 0x000000);
        assert!(above.derive(0x000000).shares_one_ink());
        assert_eq!(above.resolve().pole, 0x000000);
        assert_no_misses("black ink above the band", role_misses(&above));

        let mirrored = crossover_theme(0x686868, 0xffffff);
        assert!(!mirrored.derive(0xffffff).shares_one_ink());
        assert_eq!(mirrored.resolve().pole, 0x000000);
        assert_no_misses("white ink in the band", role_misses(&mirrored));
    }

    /// `legible_ink` picks white or black fill by fill, so on fills either
    /// side of the crossover one ink is walked back and forth and settles
    /// under the floor (2.84:1 against 3:1 here); `legible_across` fixes one
    /// direction for the whole set.
    #[test]
    fn legible_across_holds_a_floor_where_a_per_fill_direction_flips() {
        let fills = [0x777777, 0x707070, 0x6e6e6e];
        let per_fill = fills
            .iter()
            .fold(0x808080, |ink, &fill| legible_ink(fill, ink, AA_MARK));
        assert!(
            fills.iter().any(|&fill| contrast(per_fill, fill) < AA_MARK),
            "the per-fill walk {per_fill:#08x} was the failing case"
        );
        let across = legible_across(&fills, 0x808080, AA_MARK);
        for fill in fills {
            assert!(
                contrast(across, fill) >= AA_MARK,
                "{across:#08x} is {:.2}:1 on {fill:#08x}",
                contrast(across, fill)
            );
        }
    }

    /// The prescribed 65%/55% and 50%/40% seeds miss on these palettes; the
    /// cached ink is what fixes them, with the ratios on the fill that
    /// actually fails.
    #[test]
    fn settings_ink_corrects_the_prescribed_seed_on_latte_dawn_and_dark_menu_rows() {
        let by_id = |id: &str| builtins().into_iter().find(|t| t.id == id).unwrap();
        for id in ["catppuccin_latte", "rose_pine_dawn", "dark"] {
            let t = by_id(id);
            let m = t.neutrals();
            let s = t.surfaces();
            let ink = t.settings_ink();
            let seed = mix(m.background, m.foreground, if t.dark { 0.55 } else { 0.65 });
            let fail_fill = if t.dark {
                let menu = pack(lift(
                    gpui::rgb(m.background).into(),
                    settings_wash::MENU_LIFT_DARK,
                ));
                mix(menu, m.foreground, settings_wash::MENU_ROW.1)
            } else {
                s.rail.selected
            };
            assert!(
                contrast(seed, fail_fill) < AA_TEXT,
                "{id}: the prescribed seed {seed:#08x} should miss on {fail_fill:#08x}"
            );
            assert!(
                contrast(ink.sub, fail_fill) >= AA_TEXT,
                "{id}: sub {:#08x} is {:.2}:1 on {fail_fill:#08x}",
                ink.sub,
                contrast(ink.sub, fail_fill)
            );
        }
    }

    #[test]
    fn selected_rung_holds_full_text_on_every_builtin() {
        for t in builtins() {
            let s = t.surfaces();
            let fg = legible_foreground(t.background_color(), t.foreground);
            for (name, sf) in [("window", s.window), ("popover", s.popover)] {
                assert!(
                    contrast(fg, sf.selected) >= AA_TEXT,
                    "{}/{name}: full foreground is only {:.2}:1 on the selected rung {:#08x}",
                    t.id,
                    contrast(fg, sf.selected),
                    sf.selected
                );
            }
        }
        let t = builtins()
            .into_iter()
            .find(|t| t.id == "one_dark_pro")
            .unwrap();
        let fg = legible_foreground(t.background_color(), t.foreground);
        let popover = t.neutrals().popover;
        let full_step = raise(popover, fg, state::SELECTED);
        assert!(
            contrast(fg, full_step) < AA_TEXT,
            "the uncorrected One Dark Pro popover rung {full_step:#08x} was the failing case"
        );
        assert_ne!(t.surfaces().popover.selected, full_step);
    }

    /// Every surface's three rungs stay perceptibly apart in every theme, and
    /// a weak palette's selected title keeps its authored hue and saturation
    /// when it is lifted in lightness. Run with `--nocapture` to read the
    /// measured least rung delta.
    #[test]
    fn rungs_stay_ordered_and_weak_titles_keep_their_hue_in_every_theme() {
        let mut misses = Vec::new();
        let mut least = f32::MAX;
        for t in sweep_themes() {
            let s = t.surfaces();
            let fg = legible_foreground(t.background_color(), t.foreground);
            for (name, sf) in [
                ("window", s.window),
                ("popover", s.popover),
                ("sidebar", s.sidebar),
                ("rail", s.rail),
            ] {
                let hover_step = (lightness(sf.hover) - lightness(sf.base)).abs();
                let selected_step = (lightness(sf.selected) - lightness(sf.hover))
                    * (lightness(sf.hover) - lightness(sf.base)).signum();
                least = least.min(hover_step).min(selected_step);
                if hover_step < state::RUNG_MIN_DELTA || selected_step < state::RUNG_MIN_DELTA {
                    misses.push(format!(
                        "{}/{name}: rungs {:#08x} {:#08x} {:#08x} are {hover_step:.2} and {selected_step:.2} L* apart",
                        t.id, sf.base, sf.hover, sf.selected
                    ));
                }
                // The selected title is the ink actually painted on the selected
                // rung, so it clears the text floor wherever the rung is drawn.
                let on_fill = contrast(sf.text_selected, sf.selected);
                if on_fill < AA_TEXT {
                    misses.push(format!(
                        "{}/{name}: selected title {:#08x} is only {on_fill:.2}:1 on the selected rung {:#08x}",
                        t.id, sf.text_selected, sf.selected
                    ));
                }
            }
            let sf = s.window;
            let authored: Hsla = gpui::rgb(fg).into();
            let lifted: Hsla = gpui::rgb(sf.text_selected).into();
            // Eight-bit channels round a pastel's hue by a few hundredths.
            let hue_gap = (authored.h - lifted.h).abs();
            if contrast(fg, sf.selected) < AA_TEXT
                && authored.s > 0.15
                && (hue_gap.min(1. - hue_gap) > 0.05 || (authored.s - lifted.s).abs() > 0.1)
            {
                misses.push(format!(
                    "{}: lifted title {:#08x} left the hue or saturation of {fg:#08x}",
                    t.id, sf.text_selected
                ));
            }
        }
        println!("least adjacent rung delta {least:.2} L*");
        assert_no_misses("rung order and weak titles", misses);
    }

    /// The grid's inks are pastels, so its weak palettes never leave the hue
    /// and saturation check above anything to do. A saturated foreground on a
    /// dark base does: the selected rung is tinted toward the ink and leaves
    /// it near 3.5:1, so the title takes the lift. Keeping the authored hue
    /// and saturation (lightness only) is what separates `lift_lightness`
    /// from walking the ink to grey, which drops these titles' saturation by
    /// about half.
    #[test]
    fn weak_saturated_titles_keep_their_hue_and_saturation_on_the_window_selection() {
        for (background, foreground) in [
            (0x1e1e2e, 0x009b6b),
            (0x000000, 0x01864e),
            (0x2b2b2b, 0x00b027),
            (0x1e1e2e, 0x0094b1),
        ] {
            let t = Theme {
                id: format!("weak-saturated-{foreground:06x}"),
                name: format!("Weak saturated {background:06x} {foreground:06x}"),
                dark: true,
                background: Fill::Solid(background),
                foreground,
                accent: 0x1f6bf0,
                caret: None,
                selection: None,
                opacity: None,
                blur: false,
                image: None,
                ansi16: BUILTINS[0].ansi16,
                path: None,
            };
            let fg = legible_foreground(t.background_color(), t.foreground);
            assert_eq!(
                fg, foreground,
                "{}: the witness foreground is legible as authored",
                t.id
            );
            let sf = t.surfaces().window;
            assert!(
                contrast(fg, sf.selected) < AA_TEXT,
                "{}: the foreground must be weak on the selected rung {:#08x} for the lift to run",
                t.id,
                sf.selected
            );
            let authored: Hsla = gpui::rgb(fg).into();
            let lifted: Hsla = gpui::rgb(sf.text_selected).into();
            let hue_gap = (authored.h - lifted.h).abs();
            // One eight-bit channel step moves the hue of these inks (chroma
            // above 0.5) by 1/(6*255*0.5), about 0.0013, and their saturation
            // by about 0.004: the bounds allow three steps of each. Lightness
            // alone moving the ink stays inside them; walking it to grey does
            // not (saturation drops by about half).
            assert!(
                hue_gap.min(1. - hue_gap) <= 0.004 && (authored.s - lifted.s).abs() <= 0.012,
                "{}: lifted title {:#08x} left the hue or saturation of {fg:#08x}",
                t.id,
                sf.text_selected
            );
            assert!(
                contrast(sf.text_selected, sf.selected) >= AA_TEXT,
                "{}: lifted title {:#08x} is only {:.2}:1 on {:#08x}",
                t.id,
                sf.text_selected,
                contrast(sf.text_selected, sf.selected),
                sf.selected
            );
        }
    }

    /// The selected title of every surface sits at the corrected text floor on
    /// its selected rung, in every built-in and generated theme.
    #[test]
    fn selected_titles_hold_the_corrected_floor_on_their_selected_rung() {
        let mut misses = Vec::new();
        for t in sweep_themes() {
            let s = t.surfaces();
            for (name, sf) in [
                ("window", s.window),
                ("popover", s.popover),
                ("sidebar", s.sidebar),
                ("rail", s.rail),
            ] {
                let ratio = contrast(sf.text_selected, sf.selected);
                if ratio < AA_CORRECTED {
                    misses.push(format!(
                        "{}/{name}: selected title {:#08x} is {ratio:.2}:1 on {:#08x}, under 4.6",
                        t.id, sf.text_selected, sf.selected
                    ));
                }
            }
        }
        assert_no_misses("selected titles", misses);
    }

    /// A cached ink moves off its prescribed seed only where the seed misses
    /// a floor on a fill it is painted on.
    #[test]
    fn cached_inks_keep_seeds_that_already_pass() {
        let mut misses = Vec::new();
        for t in sweep_themes() {
            let r = t.resolve();
            let (m, s) = (&r.m, &r.sf);
            let fg = m.foreground;
            let settings = t.settings_ink_from(&r);
            let chrome = t.chrome_ink_from(&r);
            let (sub_share, off_share) = if t.dark { (0.55, 0.40) } else { (0.65, 0.50) };
            let popover = [s.popover.base, s.popover.hover, s.popover.selected];
            let panel = [
                s.sidebar.base,
                s.sidebar.hover,
                s.sidebar.selected,
                s.rail.base,
                s.rail.hover,
                s.rail.selected,
            ];
            assert_eq!(
                chrome.keycap_fill,
                mix(s.popover.base, r.pole, 0.06),
                "{}: keycap fill",
                t.id
            );
            // The text roles the cached-ink list below does not own: the shared caption, the
            // semantic and primary inks, the ANSI ramp and the port chip. Seeds are the inputs
            // `semantics_with`, `neutrals_at` and `interactions_with` start from.
            let sem = t.semantics_with(m);
            let inter = t.interactions_with(m);
            let clear = vec![m.background, m.sidebar, m.rail, m.popover];
            let (red, green, amber) = if t.dark {
                (0xf07878, 0x4cc27a, 0xe0a345)
            } else {
                (0xc43c43, 0x2f8a52, 0xb7791f)
            };
            let preferred_label = if t.dark { 0x121418 } else { 0xffffff };
            let mut extra: Vec<(String, u32, u32, Vec<u32>, f32)> = vec![
                (
                    "foreground".into(),
                    m.foreground,
                    t.foreground,
                    vec![m.background],
                    AA_TEXT,
                ),
                (
                    "choice_ink".into(),
                    inter.choice_ink,
                    m.foreground,
                    vec![inter.choice],
                    AA_TEXT,
                ),
                (
                    "primary on_fill".into(),
                    inter.primary.on_fill,
                    preferred_label,
                    vec![inter.primary.fill],
                    AA_TEXT,
                ),
                (
                    "caption muted_foreground".into(),
                    m.muted_foreground,
                    dim(fg, m.background, state::TEXT_RESTING),
                    r.caption.clone(),
                    AA_TEXT,
                ),
                (
                    "danger ink".into(),
                    sem.danger.ink,
                    red,
                    clear.clone(),
                    AA_TEXT,
                ),
                (
                    "success ink".into(),
                    sem.success.ink,
                    green,
                    clear.clone(),
                    AA_TEXT,
                ),
                (
                    "warning ink".into(),
                    sem.warning.ink,
                    amber,
                    clear.clone(),
                    AA_TEXT,
                ),
                (
                    "info ink".into(),
                    sem.info.ink,
                    t.accent,
                    clear.clone(),
                    AA_TEXT,
                ),
                (
                    "link ink".into(),
                    sem.link.ink,
                    t.accent,
                    clear.clone(),
                    AA_TEXT,
                ),
                (
                    "primary ink".into(),
                    inter.primary.ink,
                    m.accent,
                    clear.clone(),
                    AA_TEXT,
                ),
                (
                    "port_chip_ink".into(),
                    chrome.port_chip_ink,
                    fg,
                    r.port_chip.clone(),
                    AA_TEXT,
                ),
                (
                    "port_chip_hover_ink".into(),
                    chrome.port_chip_hover_ink,
                    sem.link.ink,
                    r.port_chip.clone(),
                    AA_TEXT,
                ),
            ];
            for (label, role) in [
                ("danger", sem.danger),
                ("success", sem.success),
                ("warning", sem.warning),
                ("info", sem.info),
                ("link", sem.link),
            ] {
                extra.push((
                    format!("{label} on_fill"),
                    role.on_fill,
                    fg,
                    vec![role.fill],
                    AA_TEXT,
                ));
            }
            for i in 0..16 {
                extra.push((
                    format!("ansi {i} ink"),
                    t.ansi_ink_with(m, i),
                    t.ansi_seed(i),
                    clear.clone(),
                    AA_TEXT,
                ));
            }
            for (what, color, seed, fills, floor) in [
                (
                    "sub",
                    settings.sub,
                    mix(m.background, fg, sub_share),
                    &r.settings_text[..],
                    AA_TEXT,
                ),
                (
                    "switch_off",
                    settings.switch_off,
                    mix(m.background, fg, off_share),
                    &r.settings_marks[..],
                    AA_MARK,
                ),
                (
                    "glyph",
                    settings.glyph,
                    mix(m.background, fg, 0.5),
                    &r.settings_glyph[..],
                    AA_MARK,
                ),
                (
                    "danger text",
                    settings.danger,
                    t.semantics_with(m).danger.ink,
                    &r.settings_status[..],
                    AA_TEXT,
                ),
                (
                    "warn_text",
                    settings.warn_text,
                    t.semantics_with(m).warning.ink,
                    &r.settings_status[..],
                    AA_TEXT,
                ),
                (
                    "ok text",
                    settings.ok,
                    t.semantics_with(m).success.ink,
                    &r.settings_status[..],
                    AA_TEXT,
                ),
                (
                    "warn mark",
                    settings.warn,
                    t.semantics_with(m).warning.fill,
                    &r.settings_warn[..],
                    AA_MARK,
                ),
                (
                    "selected_meta",
                    chrome.selected_meta,
                    mix(s.popover.selected, fg, 0.72),
                    &[s.popover.selected][..],
                    AA_TEXT,
                ),
                (
                    "quiet_name",
                    chrome.quiet_name,
                    mix(s.popover.base, fg, 0.65),
                    &popover[..],
                    AA_TEXT,
                ),
                (
                    "avatar_ink",
                    chrome.avatar_ink,
                    mix(m.secondary, fg, 0.80),
                    &[m.secondary][..],
                    AA_TEXT,
                ),
                (
                    "keycap_ink",
                    chrome.keycap_ink,
                    mix(chrome.keycap_fill, fg, 0.70),
                    &[chrome.keycap_fill][..],
                    AA_TEXT,
                ),
                (
                    "grip_idle",
                    chrome.grip_idle,
                    mix(s.window.base, fg, 0.50),
                    &[s.window.base][..],
                    AA_MARK,
                ),
                (
                    "search_title",
                    chrome.search_title,
                    mix(s.popover.base, fg, 0.92),
                    &popover[..],
                    AA_TEXT,
                ),
                ("panel_title", chrome.panel_title, fg, &panel[..], AA_TEXT),
                (
                    "unpicked_title",
                    chrome.unpicked_title,
                    fg,
                    &[s.popover.base, s.popover.hover][..],
                    AA_TEXT,
                ),
                (
                    "unpicked_meta",
                    chrome.unpicked_meta,
                    m.muted_foreground,
                    &[s.popover.base, s.popover.hover][..],
                    AA_TEXT,
                ),
            ]
            .into_iter()
            .map(|(what, color, seed, fills, floor)| {
                (what.to_string(), color, seed, fills.to_vec(), floor)
            })
            .chain(extra)
            {
                if fills.iter().all(|&fill| contrast(seed, fill) >= floor) && color != seed {
                    misses.push(format!(
                        "{}: {what} moved off a seed {seed:#08x} that already passes",
                        t.id
                    ));
                }
                // A text role that did have to move lands at the 4.6 target on every fill, so
                // antialiasing cannot take it back under 4.5. Reported as exact cases, never
                // excluded: an infeasible one shows up here with its ratio.
                if floor == AA_TEXT && color != seed {
                    for &fill in &fills {
                        if contrast(color, fill) < AA_CORRECTED {
                            misses.push(format!(
                                "{}: corrected {what} {color:#08x} is {:.2}:1 on {fill:#08x}, under the 4.6 target",
                                t.id,
                                contrast(color, fill)
                            ));
                        }
                    }
                }
            }
        }
        assert_no_misses("seeds kept", misses);
    }
}
